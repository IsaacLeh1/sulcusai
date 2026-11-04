// SPDX-License-Identifier: AGPL-3.0-only
//! Video clips, long videos, captions, and pictures that talk.

use super::*;

pub(super) async fn video(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let d = &m.defaults;
    let fps = d.fps.max(8);
    // No longer than the model was trained on (Wan 2.2: 121 frames, Wan 2.1: 81).
    let max_frames = if d.max_frames > 0 { d.max_frames } else { 81 };
    let longest = (max_frames as f32 - 1.0) / fps as f32;
    // Longer: in parts, each going on from the last frame of the one before
    // (models that start from a picture; the parts are joined on Windows).
    // A clip with its own sound is made in one go (parts would cut the sound up).
    let sound = m.file("audio_vae").is_some();
    // Ending on a picture is one clip: a long one would end its first part there.
    let can_continue = m.can("image") && cfg!(windows) && req.wallpaper.is_none() && !sound && req.end.is_none();
    let most = if can_continue { longclip::MAX_SECS } else { longest };
    let seconds = req.seconds.unwrap_or(if req.wallpaper.is_some() { longest } else { d.seconds }).clamp(1.0, most.max(1.0));
    let total = (seconds * fps as f32).round() as u32 + 1;
    let parts = match () {
        _ if can_continue => longclip::plan(total, max_frames),
        // LTX: 8n+1 frames.
        _ if sound => vec![sd::frames_in_steps(seconds, fps, 8).min(max_frames)],
        _ => vec![sd::frames_for(seconds, fps).min(max_frames)],
    };
    let frames = parts[0];
    let (mut w, mut h) = (d.width.max(256), d.height.max(256));
    if let Some(spec) = &req.wallpaper {
        (w, h) = wallpaper::make_size(spec, imaging::round16(((w * h) as f64).sqrt()), 32);
    } else if let (Some(rw), Some(rh)) = (req.width, req.height) {
        // A size asked for, in the steps of 32 the video models work in.
        let step = |v: u32| (v.clamp(256, 1920) + 16) / 32 * 32;
        (w, h) = (step(rw), step(rh));
    } else if matches!(req.shape.as_deref(), Some("portrait" | "tall")) {
        std::mem::swap(&mut w, &mut h);
    } else if req.shape.as_deref() == Some("square") {
        let s = imaging::round16(((w * h) as f64).sqrt());
        (w, h) = (s, s);
    }
    let mut parent = None;
    // A clip made from a picture starts on it: that's its thumbnail.
    let mut still = None;
    let init = match req.source.as_deref() {
        Some(_) => {
            let (item, img) = ctx.source()?;
            // Same shape as the picture, at the model's size.
            let (iw, ih) = img.dimensions();
            let area = (w * h) as f64;
            let k = (area / (iw as f64 * ih as f64)).sqrt();
            // Within what the models take (a wide panorama asked for 2832x144 and failed after loading).
            (w, h) = (imaging::round16(iw as f64 * k).clamp(256, 1920), imaging::round16(ih as f64 * k).clamp(256, 1920));
            let p = work.join("start.png");
            let start = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
            std::fs::write(&p, imaging::png(&start)?).map_err(|e| e.to_string())?;
            still = Some(imaging::thumbnail(&start)?);
            parent = Some(item.id);
            Some(p)
        }
        None => None,
    };
    let end = match req.end.as_deref() {
        Some(id) => {
            // Only first-and-last-frame models take it: Wan 2.2 TI2V ignored
            // the end picture and LTX-2.3 came out blank.
            if init.is_none() || !m.can("end") {
                return Err(format!("{} can't end a clip on a picture; that needs a first-and-last-frame model.", m.name));
            }
            let c = ctx.state.work_cipher()?;
            let img = imaging::decode(&store::read(&ctx.state.paths, &c, id)?)?;
            let p = work.join("end.png");
            std::fs::write(&p, imaging::png(&img.resize_exact(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
            Some(p)
        }
        None => None,
    };
    let mut prompt = if req.prompt.trim().is_empty() { "Gentle natural motion, the scene comes to life.".to_string() } else { req.prompt.trim().to_string() };
    if let Some(words) = req.camera.as_deref().and_then(camera_words) {
        prompt = format!("{}, {words}", prompt.trim_end_matches(['.', ' ']));
    }
    if let Some(spec) = req.wallpaper.as_ref().filter(|_| req.style.as_deref() != Some("as_written")) {
        prompt.push_str(wallpaper::plain_words(spec));
        prompt.push_str(", slow continuous gentle motion, camera nearly still");
    }
    let seed = req.seed.unwrap_or_else(random_seed);
    if parts.len() > 1 {
        return long_video(ctx, m, work, LongClip { prompt: &prompt, parts: &parts, total, fps, w, h, seed, init, parent, still }).await;
    }
    // A wallpaper's frames come out as pictures, to be fitted to the screen and looped.
    let out = if req.wallpaper.is_some() && cfg!(windows) { work.join("frame_%03d.png") } else { work.join("out.webm") };
    let args = sd::clip_args(
        &ctx.files(m),
        d,
        &sd::Clip {
            prompt: &prompt,
            negative: req.negative.as_deref(),
            width: w,
            height: h,
            frames,
            fps,
            steps: req.steps.unwrap_or(d.steps).clamp(4, 50),
            seed,
            init: init.as_deref(),
            audio: None,
            end: end.as_deref(),
            out: &out,
        },
        &ctx.limits(),
    );
    ctx.run_sd(args, work).await?;
    let c = ctx.state.work_cipher()?;
    if let Some(spec) = req.wallpaper.as_ref().filter(|_| cfg!(windows)) {
        let files = wallpaper::frame_files(work);
        (ctx.progress)("Fitting it to your screen and making it loop", 0.0);
        let mp4 = work.join("wallpaper.mp4");
        let (bytes, poster) = tokio::task::block_in_place(|| wallpaper::encode_loop(&files, spec, fps, &mp4, ctx.cancel, &|f| (ctx.progress)("Fitting it to your screen and making it loop", f)))?;
        let (cw, ch) = wallpaper::clip_size(spec);
        let shown = wallpaper::loop_plan(files.len()).len();
        let item = MediaItem {
            kind: "video".into(),
            op: "wallpaper".into(),
            prompt: req.asked.as_deref().unwrap_or(&req.prompt).trim().to_string(),
            model_id: Some(m.id.clone()),
            mime: "video/mp4".into(),
            width: cw,
            height: ch,
            seconds: shown as f64 / fps as f64,
            chat_id: req.chat_id.clone(),
            seed: Some(seed),
            poster: true,
            ..Default::default()
        };
        let thumb = imaging::thumbnail(&poster)?;
        return Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &bytes, Some(&thumb))?]);
    }
    let (out, mime) = if sound { with_its_sound(ctx, &out, work).await? } else { (out, "video/webm") };
    let bytes = std::fs::read(&out).map_err(|_| "The video engine didn't produce a clip.".to_string())?;
    let item = MediaItem {
        kind: "video".into(),
        op: "video".into(),
        prompt: req.asked.as_deref().unwrap_or(&req.prompt).trim().to_string(),
        model_id: Some(m.id.clone()),
        mime: mime.into(),
        width: w,
        height: h,
        seconds: frames as f64 / fps as f64,
        parent,
        chat_id: req.chat_id.clone(),
        seed: Some(seed),
        poster: still.is_some(),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &bytes, still.as_deref())?])
}

/// Camera moves the window offers, as the words video models follow.
pub const CAMERA: &[(&str, &str)] = &[
    ("static", "static shot, the camera stays completely still"),
    ("pan_left", "the camera slowly pans to the left"),
    ("pan_right", "the camera slowly pans to the right"),
    ("tilt_up", "the camera slowly tilts up"),
    ("tilt_down", "the camera slowly tilts down"),
    ("zoom_in", "the camera slowly zooms in"),
    ("zoom_out", "the camera slowly zooms out"),
    ("dolly_in", "the camera moves forward toward the subject (dolly in)"),
    ("dolly_out", "the camera pulls back away from the subject (dolly out)"),
    ("orbit", "the camera orbits around the subject"),
    ("tracking", "tracking shot, the camera follows the subject as it moves"),
    ("crane_up", "crane shot, the camera rises up and over the scene"),
    ("handheld", "handheld camera with slight natural shake"),
    ("drone", "aerial drone shot, the camera glides over the scene"),
];

pub(crate) fn camera_words(id: &str) -> Option<&'static str> {
    CAMERA.iter().find(|(k, _)| *k == id).map(|(_, w)| *w)
}

/// A clip that came with its own sound (LTX): the engine puts it in the
/// WebM as raw PCM, which the window's player can't play, so it's made an
/// MP4 with AAC. A clip whose sound didn't come out (the engine then writes
/// a silent one) is kept as it is, and the log says so.
pub(super) async fn with_its_sound(ctx: &RunCtx<'_>, webm: &std::path::Path, work: &std::path::Path) -> Result<(PathBuf, &'static str), String> {
    let state = ctx.state;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let probe = ffmpeg::probe(&ffprobe, webm).await?;
    if !probe.audio {
        state.log("media", "The video model's sound didn't come out; the clip is silent");
        return Ok((webm.to_path_buf(), "video/webm"));
    }
    (ctx.progress)("Putting the sound with it", 0.97);
    let mp4 = work.join("with-sound.mp4");
    let mut args: Vec<std::ffi::OsString> = vec!["-i".into(), webm.as_os_str().to_owned()];
    args.extend(ffmpeg::mp4_out(&mp4));
    ffmpeg::run(state, &ffmpeg_exe, args, probe.seconds, ctx.cancel, &|f| (ctx.progress)("Putting the sound with it", 0.97 + 0.03 * f)).await?;
    Ok((mp4, "video/mp4"))
}

pub(super) struct LongClip<'a> {
    pub(super) prompt: &'a str,
    /// Frames for each part.
    pub(super) parts: &'a [u32],
    /// Frames in the finished clip.
    pub(super) total: u32,
    pub(super) fps: u32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) seed: i64,
    /// The picture it starts from, if any.
    pub(super) init: Option<PathBuf>,
    pub(super) parent: Option<String>,
    pub(super) still: Option<Vec<u8>>,
}

/// A clip longer than the model makes at once: part after part, each from
/// the last frame of the one before (with a rest when the PC runs hot),
/// then joined into one MP4.
pub(super) async fn long_video(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path, c: LongClip<'_>) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let n = c.parts.len();
    let mut start = c.init.clone();
    let mut made: Vec<Vec<PathBuf>> = Vec::new();
    for (i, frames) in c.parts.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        if i > 0 {
            crate::perf::cool_down(&ctx.state.limits(), |what| (ctx.progress)(&format!("Letting the {what} cool down before part {}", i + 1), i as f64 / n as f64)).await;
        }
        // The engine reports each of its stages from 0 to 1; across the whole
        // clip, loading counts as the part's start and creating as most of it
        // (loading at 100% read as half the clip done, "10 s left").
        let stage = |s: &str, f: f64| {
            let f = f.clamp(0.0, 1.0);
            let within = match s {
                "Loading the model" => 0.0,
                "Creating" => f * 0.92,
                _ => 0.92 + f * 0.08,
            };
            (ctx.progress)(&format!("Part {} of {n}: {s}", i + 1), ((i as f64 + within) / n as f64) * 0.97)
        };
        let part_ctx = RunCtx { state: ctx.state, req, cancel: ctx.cancel, progress: &stage, reserved: ctx.reserved };
        let dir = work.join(format!("part{i:03}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let out = dir.join("f_%04d.png");
        let args = sd::clip_args(
            &ctx.files(m),
            &m.defaults,
            &sd::Clip {
                prompt: c.prompt,
                negative: req.negative.as_deref(),
                width: c.w,
                height: c.h,
                frames: *frames,
                fps: c.fps,
                steps: req.steps.unwrap_or(m.defaults.steps).clamp(4, 50),
                seed: c.seed + i as i64,
                init: start.as_deref(),
                audio: None,
                end: None,
                out: &out,
            },
            &part_ctx.limits(),
        );
        part_ctx.run_sd(args, &dir).await?;
        let files = longclip::frames_in(&dir, "f_");
        let last = files.last().cloned().ok_or("The video engine didn't produce any frames.")?;
        crate::applog::line("media", &format!("Long clip: part {} of {n} made ({} frames)", i + 1, files.len()));
        start = Some(last);
        made.push(files);
    }
    (ctx.progress)("Joining the parts into one video", 0.97);
    let mp4 = work.join("clip.mp4");
    let (bytes, first) = tokio::task::block_in_place(|| longclip::join(&made, c.total as usize, c.fps, &mp4, ctx.cancel, &|f| (ctx.progress)("Joining the parts into one video", 0.97 + f * 0.03)))?;
    let thumb = match c.still {
        Some(s) => s,
        None => imaging::thumbnail(&first)?,
    };
    let cipher = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "video".into(),
        op: "video".into(),
        prompt: req.asked.as_deref().unwrap_or(&req.prompt).trim().to_string(),
        model_id: Some(m.id.clone()),
        mime: "video/mp4".into(),
        width: first.width() / 2 * 2,
        height: first.height() / 2 * 2,
        seconds: (c.total.saturating_sub(1)) as f64 / c.fps as f64,
        parent: c.parent,
        chat_id: req.chat_id.clone(),
        seed: Some(c.seed),
        poster: true,
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &cipher, item, &bytes, Some(&thumb))?])
}

/// Caption lines from what was heard: a long stretch is split by words,
/// in proportion to its time, so no line runs past about 7 s or 84 letters.
pub(crate) fn caption_lines(segments: &[crate::speech::Segment]) -> Vec<(f64, f64, String)> {
    let mut out = Vec::new();
    for s in segments {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        let len = (s.end - s.start).max(0.3);
        let pieces = ((text.chars().count() as f64 / 84.0).ceil().max((len / 7.0).ceil())).max(1.0) as usize;
        let per = words.len().div_ceil(pieces).max(1);
        let total = words.len().max(1) as f64;
        for (i, chunk) in words.chunks(per).enumerate() {
            let a = s.start + len * (i * per) as f64 / total;
            let b = s.start + len * ((i * per + chunk.len()) as f64 / total);
            out.push((a, b, chunk.join(" ")));
        }
    }
    out
}

/// "00:01:02,345" for a subtitle file.
pub(super) fn srt_time(secs: f64) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

pub(crate) fn srt(lines: &[(f64, f64, String)]) -> String {
    lines.iter().enumerate().map(|(i, (a, b, t))| format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(*a), srt_time(*b), t)).collect()
}

/// A file's path as FFmpeg's filters read it ("C\:/Users/…": the drive's
/// colon escaped, forward slashes).
pub(super) fn filter_path(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "\\'")
}

/// A video with what's said in it as captions, in its own language or
/// translated (`language`): heard by whisper, cut into lines a viewer can
/// read, and drawn onto a new copy. The captions' text is kept with it.
pub(super) async fn captions(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let video = written_out(state, &c, &item, &work.join("source"))?;
    let probe = ffmpeg::probe(&ffprobe, &video).await?;
    if !probe.audio {
        return Err("This video has no sound, so there's nothing to caption.".into());
    }
    (ctx.progress)("Listening to what's said", 0.05);
    let mix = ffmpeg_stereo(state, &ffmpeg_exe, &video, probe.seconds, ctx.cancel).await?;
    let speech = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.map_err(|e| format!("Captions need a speech model to hear what's said. {e}"))?;
    let options = crate::speech::Options { language: Some("auto".into()), ..Default::default() };
    let heard = crate::speech::transcribe(&speech, &stems::resample(&mix.mono(), stems::RATE, crate::audio::RATE), &options).await?;
    // Whole sentences (whisper breaks mid-sentence), translated as wholes,
    // then cut into lines: cut first, "...the green pigment chlorophyll in
    // its" and "leaves captures sunlight" were translated apart.
    let mut sentences: Vec<crate::speech::Segment> =
        dub_lines(crate::speech::clean(heard).segments).into_iter().map(|(start, end, text)| crate::speech::Segment { start, end, text }).collect();
    if sentences.is_empty() {
        return Err("No one speaks in this video, so there's nothing to caption.".into());
    }
    let to = ctx.req.language.clone().filter(|l| !l.is_empty() && l != "auto" && l != "same");
    if let Some(to) = &to {
        (ctx.progress)("Translating the captions", 0.35);
        let said: Vec<String> = sentences.iter().map(|s| s.text.clone()).collect();
        let (ep, _, _hold) = crate::background_endpoint(state).await.map_err(|e| format!("Translating the captions needs a chat model: {e}"))?;
        let words = crate::translate::translate_lines(&ep, &said, to).await?;
        for (s, w) in sentences.iter_mut().zip(words) {
            s.text = w;
        }
    }
    let lines = caption_lines(&sentences);
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let text = srt(&lines);
    let file = work.join("captions.srt");
    std::fs::write(&file, &text).map_err(|e| e.to_string())?;
    // Big enough to read on a phone, white with a dark edge, near the bottom.
    let style = "FontName=Arial,FontSize=15,PrimaryColour=&H00FFFFFF,OutlineColour=&H00000000,BorderStyle=1,Outline=1.6,Shadow=0,MarginV=22";
    let out = work.join("captioned.mp4");
    let mut args: Vec<std::ffi::OsString> = ["-i".into(), video.as_os_str().to_owned()].into();
    args.extend(["-vf".into(), format!("subtitles='{}':force_style='{style}'", filter_path(&file)).into(), "-map".into(), "0:v".into(), "-map".into(), "0:a?".into()]);
    args.extend(ffmpeg::mp4_out(&out));
    (ctx.progress)("Drawing the captions", 0.5);
    ffmpeg::run(state, &ffmpeg_exe, args, probe.seconds, ctx.cancel, &|f| (ctx.progress)("Drawing the captions", 0.5 + 0.45 * f)).await?;
    let still = work.join("still.jpg");
    let _ = ffmpeg::run(state, &ffmpeg_exe, ["-ss", &format!("{:.2}", (probe.seconds / 3.0).min(2.0)), "-i"].iter().map(Into::into).chain([out.as_os_str().to_owned(), "-frames:v".into(), "1".into(), "-q:v".into(), "3".into(), still.as_os_str().to_owned()]).collect(), 0.0, ctx.cancel, &|_| {}).await;
    let thumb = std::fs::read(&still).ok().and_then(|b| imaging::decode(&b).ok()).and_then(|img| imaging::thumbnail(&img).ok());
    let bytes = std::fs::read(&out).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let made = MediaItem {
        kind: "video".into(),
        op: "captions".into(),
        prompt: item.prompt.clone(),
        model_id: item.model_id.clone(),
        mime: "video/mp4".into(),
        seconds: probe.seconds,
        width: probe.width,
        height: probe.height,
        parent: Some(item.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| item.chat_id.clone()),
        lyrics: Some(text),
        poster: thumb.is_some(),
        ..Default::default()
    };
    state.log("media", &format!("Captioned a video on this PC ({} lines{})", lines.len(), to.as_deref().map(|l| format!(", in {}", crate::translate::language_name(l))).unwrap_or_default()));
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, made, &bytes, thumb.as_deref())?])
}

/// Frames in each part of a talking picture: Wan 2.2 S2V makes about 5 s at
/// once (77 frames at its 16 a second).
pub(super) const SPEAK_FRAMES: u32 = 77;
/// The longest voice a picture talks or sings to (each 5 s takes minutes).
pub(super) const SPEAK_SECS: f64 = 60.0;

/// The parts of a mix HTDemucs was asked for (`wanted`: 0 drums, 1 bass,
/// 2 the rest, 3 vocals), added together; None when the separator isn't
/// there or fails (said in the log), so the caller makes do without.
pub(super) async fn separated(ctx: &RunCtx<'_>, mix: &stems::Stereo, wanted: &[usize], why: &str) -> Result<Option<[Vec<f32>; 2]>, String> {
    let state = ctx.state;
    let Some(sep) = state.catalog.media.separator.clone() else { return Ok(None) };
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    let threads = state.limits().threads;
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let apart = tokio::task::block_in_place(|| {
        crate::cardfirst::asking(ask, || {
            let r = stems::separate(&dll, &separator, mix, threads, wanted, card, ctx.cancel);
            stems::unload();
            r
        })
    });
    match apart {
        Ok((parts, _)) => Ok(Some([0, 1].map(|ch| (0..mix.len()).map(|s| parts.iter().map(|p| p.ch[ch].get(s).copied().unwrap_or(0.0)).sum::<f32>()).collect::<Vec<f32>>()))),
        Err(e) if e == download::CANCELLED => Err(e),
        Err(e) => {
            state.log("media", &format!("The part separator couldn't take the song apart ({e}); {why}"));
            Ok(None)
        }
    }
}

/// A picture that talks or sings (Wan 2.2 S2V): its face and lips move to
/// a voice, 5 s at a time (each part starting from the last frame of the
/// one before), then FFmpeg joins the frames and lays the sound under
/// them. The voice is a sound or song from the gallery (a song's lips
/// follow its vocals alone), a file, or words read aloud first.
pub(super) async fn speak(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let req = ctx.req;
    let c = state.work_cipher()?;
    let (picture, img) = ctx.source()?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let said = req.say.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ToSay { sound, secs, voice, song } = what_it_says(ctx, work, &ffmpeg_exe, &ffprobe, SPEAK_SECS).await?;

    // Its own engine build, when it has one (its 4-step LoRA needs it).
    let own = match &m.engine {
        Some(e) => Some(own_engine(state, e, ctx.cancel).await?),
        None => None,
    };

    // Its picture, at the model's size in its own shape.
    let d = &m.defaults;
    let fps = 16;
    let (iw, ih) = img.dimensions();
    let k = ((d.width.max(256) * d.height.max(256)) as f64 / (iw as f64 * ih as f64)).sqrt();
    let (w, h) = (imaging::round16(iw as f64 * k).clamp(256, 1280), imaging::round16(ih as f64 * k).clamp(256, 1280));
    let first = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let first_path = work.join("start.png");
    std::fs::write(&first_path, imaging::png(&first)?).map_err(|e| e.to_string())?;
    let still = imaging::thumbnail(&first)?;
    let prompt = match req.prompt.trim() {
        "" if song => "A person singing with feeling, lips moving with the words, natural expressions and slight head movement.",
        "" => "A person talking naturally, lips moving with the words, natural expressions and slight head movement.",
        p => p,
    };
    let seed = req.seed.unwrap_or_else(random_seed);
    let total = (secs * fps as f64).round() as u32 + 1;
    let parts = longclip::plan(total, SPEAK_FRAMES);
    let n = parts.len();
    let sr = stems::RATE as f64;
    let mut start = Some(first_path);
    let mut made: Vec<Vec<PathBuf>> = Vec::new();
    let mut at = 0u32;
    for (i, frames) in parts.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        if i > 0 {
            crate::perf::cool_down(&state.limits(), |what| (ctx.progress)(&format!("Letting the {what} cool down before part {}", i + 1), 0.05 + 0.9 * i as f64 / n as f64)).await;
        }
        // This part's stretch of the voice (the frame it starts on is the
        // last one of the part before).
        let from = ((at as f64 / fps as f64) * sr) as usize;
        let to = (((at + frames) as f64 / fps as f64) * sr) as usize;
        let piece = voice.get(from.min(voice.len())..to.min(voice.len())).unwrap_or_default();
        let wav = work.join(format!("voice{i:03}.wav"));
        std::fs::write(&wav, crate::audio::wav_encode(piece, stems::RATE)).map_err(|e| e.to_string())?;
        let stage = |s: &str, f: f64| {
            let within = match s {
                "Loading the model" => 0.0,
                "Creating" => f.clamp(0.0, 1.0) * 0.92,
                _ => 0.92 + f.clamp(0.0, 1.0) * 0.08,
            };
            let label = if n > 1 { format!("Part {} of {n}: {s}", i + 1) } else { s.to_string() };
            (ctx.progress)(&label, 0.05 + 0.9 * (i as f64 + within) / n as f64)
        };
        let part_ctx = RunCtx { state, req, cancel: ctx.cancel, progress: &stage, reserved: ctx.reserved };
        let dir = work.join(format!("part{i:03}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let out = dir.join("f_%04d.png");
        // Weights streamed to the card, without the engine's own memory cap (`streamed_only`).
        let args = without_vram(&sd::clip_args(
            &ctx.files(m),
            d,
            &sd::Clip { prompt, negative: req.negative.as_deref(), width: w, height: h, frames: *frames, fps, steps: req.steps.unwrap_or(d.steps).clamp(2, 50), seed: seed + i as i64, init: start.as_deref(), audio: Some(&wav), end: None, out: &out },
            &part_ctx.limits(),
        ));
        match &own {
            Some(exe) => SD_ENGINE.scope(exe.clone(), part_ctx.run_sd(args, &dir)).await?,
            None => part_ctx.run_sd(args, &dir).await?,
        }
        let files = longclip::frames_in(&dir, "f_");
        start = Some(files.last().cloned().ok_or("The video engine didn't produce any frames.")?);
        crate::applog::line("media", &format!("Talking picture: part {} of {n} made ({} frames)", i + 1, files.len()));
        made.push(files);
        at += frames - 1;
    }

    // The frames in order (each later part's first is the last one before), then the sound under them.
    (ctx.progress)("Putting the sound with it", 0.96);
    let seq = work.join("seq");
    std::fs::create_dir_all(&seq).map_err(|e| e.to_string())?;
    let mut k = 0usize;
    for (i, part) in made.iter().enumerate() {
        for f in part.iter().skip(usize::from(i > 0)) {
            if k >= total as usize {
                break;
            }
            let to = seq.join(format!("{k:05}.png"));
            std::fs::hard_link(f, &to).or_else(|_| std::fs::copy(f, &to).map(|_| ())).map_err(|e| e.to_string())?;
            k += 1;
        }
    }
    let mp4 = work.join("talking.mp4");
    let mut args: Vec<std::ffi::OsString> = ["-framerate".into(), fps.to_string().into(), "-i".into(), seq.join("%05d.png").into_os_string(), "-i".into(), sound.as_os_str().to_owned()].into();
    // S2V makes 16 frames a second, which looked choppy: frames made in
    // between from the motion around them double it (a 6 s clip in about 4 s).
    args.extend(["-vf", "minterpolate=fps=32:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1"].map(Into::into));
    args.extend(["-map", "0:v", "-map", "1:a", "-t", &format!("{secs:.3}")].map(Into::into));
    args.extend(ffmpeg::mp4_out(&mp4));
    ffmpeg::run(state, &ffmpeg_exe, args, secs, ctx.cancel, &|f| (ctx.progress)("Putting the sound with it", 0.96 + 0.04 * f)).await?;
    let bytes = std::fs::read(&mp4).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let item = MediaItem {
        kind: "video".into(),
        op: "speak".into(),
        prompt: said.map(str::to_string).unwrap_or_else(|| req.prompt.trim().to_string()),
        model_id: Some(m.id.clone()),
        mime: "video/mp4".into(),
        width: w,
        height: h,
        seconds: secs,
        parent: Some(picture.id),
        chat_id: req.chat_id.clone(),
        seed: Some(seed),
        poster: true,
        ..Default::default()
    };
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &bytes, Some(&still))?])
}

/// What a picture says or sings: the sound (as given), how long it is (at
/// most `longest`), and the voice alone at `stems::RATE`.
pub(super) struct ToSay {
    pub(super) sound: PathBuf,
    pub(super) secs: f64,
    pub(super) voice: Vec<f32>,
    pub(super) song: bool,
}

/// The voice a picture talks or sings to: a sound or song from the gallery
/// (a song's lips follow its vocals alone), a file, or words read aloud first.
pub(super) async fn what_it_says(ctx: &RunCtx<'_>, work: &std::path::Path, ffmpeg_exe: &std::path::Path, ffprobe: &std::path::Path, longest: f64) -> Result<ToSay, String> {
    let state = ctx.state;
    let req = ctx.req;
    let c = state.work_cipher()?;
    let said = req.say.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let mut song = false;
    let sound = match (&req.sound, &req.sound_file, said) {
        (Some(id), _, _) => {
            let a = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That sound is no longer in the gallery.")?;
            song = matches!(a.op.as_str(), "music" | "remake" | "revoice" | "studio_part");
            written_out(state, &c, &a, &work.join("sound"))?
        }
        (None, Some(f), _) => PathBuf::from(f),
        (None, None, Some(words)) => {
            (ctx.progress)("Reading the words aloud", 0.0);
            let text = crate::tts::speakable(words);
            let language = req.language.clone().filter(|l| !l.is_empty() && l != "auto");
            let (floats, rate) = match req.voice.as_deref().and_then(|v| v.strip_prefix("own:")) {
                Some(own) => own_voice::speak(state, own, &text, language.as_deref(), ctx.cancel, &|s, f| (ctx.progress)(s, 0.03 * f)).await?,
                None => tokio::task::block_in_place(|| -> Result<(Vec<f32>, u32), String> {
                    let voices = crate::tts::voices()?;
                    let voice = crate::tts::pick_voice(&voices, req.voice.as_deref(), language.as_deref()).map(|v| v.id.clone()).ok_or("No voices are installed.")?;
                    let s = crate::tts::synthesize(&text, Some(&voice), language.as_deref(), 1.0)?;
                    Ok((s.samples.iter().map(|x| *x as f32 / 32768.0).collect(), s.rate))
                })?,
            };
            let p = work.join("sound.wav");
            std::fs::write(&p, crate::audio::wav_encode(&floats, rate)).map_err(|e| e.to_string())?;
            p
        }
        _ => return Err("Pick what it should say or sing: a sound or song, a file, or words to read aloud.".into()),
    };
    let probe = ffmpeg::probe(ffprobe, &sound).await?;
    if !probe.audio {
        return Err("That file has no sound to talk to.".into());
    }
    let secs = probe.seconds.clamp(0.5, longest);
    let mix = ffmpeg_stereo(state, ffmpeg_exe, &sound, secs, ctx.cancel).await?;
    // A song's lips follow its singing, not the band.
    let voice = match song {
        true => {
            (ctx.progress)("Finding the singing in the song", 0.03);
            separated(ctx, &mix, &[3], "the lips follow the whole song").await?.map(|[l, r]| l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect())
        }
        false => None,
    }
    .unwrap_or_else(|| mix.mono());
    Ok(ToSay { sound, secs, voice, song })
}

/// A picture that talks or sings, the quick way (`talk`, Ditto): its face
/// and lips move to the voice at 25 frames a second in one go, then FFmpeg
/// lays the sound under the video. Saved as a talking picture (`speak`).
#[cfg(windows)]
pub(super) async fn talk(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let req = ctx.req;
    let d = state.catalog.media.ditto.clone().ok_or("Quick talking pictures aren't available on this system yet.")?;
    let c = state.work_cipher()?;
    let (picture, img) = ctx.source()?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let said = req.say.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ToSay { sound, secs, voice, .. } = what_it_says(ctx, work, &ffmpeg_exe, &ffprobe, ditto::LONGEST).await?;
    let processor = !crate::natural::nvidia_here(state) || ON_PROCESSOR.try_with(|p| *p).unwrap_or(false);
    let uv = ensure_ditto(ctx, &d, processor).await?;

    // The picture as it is (at most 1920 on its longest side), and the voice alone.
    let (w, h) = ditto::picture_size(img.width(), img.height());
    let first = if (w, h) == img.dimensions() { img.clone() } else { img.resize_exact(w, h, image::imageops::FilterType::Lanczos3) };
    let image = work.join("face.png");
    std::fs::write(&image, imaging::png(&first)?).map_err(|e| e.to_string())?;
    let still = imaging::thumbnail(&first)?;
    let wav = work.join("voice.wav");
    std::fs::write(&wav, crate::audio::wav_encode(&voice, stems::RATE)).map_err(|e| e.to_string())?;
    let silent = work.join("silent.mp4");

    (ctx.progress)("Loading the face model", 0.1);
    let mut frames = (secs * 25.0).ceil().max(1.0) as u32;
    let mut no_face = false;
    let ran = proc::run_env(
        proc::Run {
            exe: &uv,
            args: ditto::args(&ditto::source_dir(&state.paths, &d), &ditto::models_dir(&state.paths), &image, &wav, &silent, req.steps.unwrap_or(50), processor),
            cwd: work,
            log: &state.paths.logs.join("talking-picture.log"),
            low_priority: state.limits().low_priority,
            job: state.job(),
        },
        &ditto::env(&state.paths),
        ctx.cancel,
        |l| match ditto::said(l) {
            Some(ditto::Said::Loaded) => (ctx.progress)("Moving the face to the voice", 0.15),
            Some(ditto::Said::Frames(n)) => frames = n.max(1),
            Some(ditto::Said::Frame(k)) => (ctx.progress)(&format!("Making the frames: {k} of {frames}"), 0.15 + 0.78 * (k as f64 / frames as f64).min(1.0)),
            Some(ditto::Said::NoFace) => no_face = true,
            None => {}
        },
    )
    .await;
    if no_face {
        return Err("No face was found in the picture. Pick one where a face is clearly seen from the front.".into());
    }
    ran?;

    (ctx.progress)("Putting the sound with it", 0.94);
    let mp4 = work.join("talking.mp4");
    let mut args: Vec<std::ffi::OsString> = vec!["-i".into(), silent.into_os_string(), "-i".into(), sound.as_os_str().to_owned()];
    args.extend(["-map", "0:v", "-map", "1:a", "-t", &format!("{secs:.3}"), "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"].map(Into::into));
    args.push(mp4.as_os_str().to_owned());
    ffmpeg::run(state, &ffmpeg_exe, args, secs, ctx.cancel, &|f| (ctx.progress)("Putting the sound with it", 0.94 + 0.06 * f)).await?;
    let bytes = std::fs::read(&mp4).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let item = MediaItem {
        kind: "video".into(),
        op: "speak".into(),
        prompt: said.map(str::to_string).unwrap_or_else(|| req.prompt.trim().to_string()),
        model_id: Some("ditto".into()),
        mime: "video/mp4".into(),
        width: w,
        height: h,
        seconds: secs,
        parent: Some(picture.id),
        chat_id: req.chat_id.clone(),
        poster: true,
        ..Default::default()
    };
    let on = if processor { "on the processor" } else { "on the graphics card" };
    state.log("media", &format!("Made a picture talk the quick way {on} ({secs:.0} s)"));
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &bytes, Some(&still))?])
}

/// Ditto, set up the first time (`talk`): uv (the music trainer's), its
/// source with SulcusAI's runner, its locked Python packages (for the card,
/// or for the `processor`), and its model files. Returns uv.
#[cfg(not(windows))]
pub(super) async fn ensure_ditto(_ctx: &RunCtx<'_>, _d: &ditto::DittoCatalog, _processor: bool) -> Result<PathBuf, String> {
    Err("Quick talking pictures (Ditto) run on Windows only so far.".into())
}

#[cfg(windows)]
pub(super) async fn ensure_ditto(ctx: &RunCtx<'_>, d: &ditto::DittoCatalog, processor: bool) -> Result<PathBuf, String> {
    let state = ctx.state;
    let paths = &state.paths;
    let say = "Getting the quick face model (once)";
    let uv = engine::ensure_unpacked(paths, "trainer-uv-bin", &d.uv, "uv.exe", ctx.cancel, |r, all| (ctx.progress)(say, 0.01 * r as f64 / all.max(1) as f64)).await?;
    let source = ditto::source_dir(paths, d);
    let dir_name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
    engine::ensure_unpacked(paths, &dir_name, &d.source, "talk.py", ctx.cancel, |_, _| {}).await?;
    let project = ditto::project_dir(&source, processor);
    if !project.join(".venv").join("Scripts").join("python.exe").is_file() {
        if processor {
            state.log("network", "Downloading the quick face model's Python packages from pypi.org");
            (ctx.progress)("Setting up the quick face model (once, about 1 GB)", 0.02);
        } else {
            state.log("network", "Downloading the quick face model's Python packages from pypi.org and download.pytorch.org");
            (ctx.progress)("Setting up the quick face model (once, up to 6 GB)", 0.02);
        }
        let ran = proc::run_env(
            proc::Run {
                exe: &uv,
                args: ["sync", "--frozen", "--python", "3.12"].map(std::ffi::OsString::from).to_vec(),
                cwd: &project,
                log: &paths.logs.join("talking-picture-setup.log"),
                low_priority: state.limits().low_priority,
                job: state.job(),
            },
            &ditto::env(paths),
            ctx.cancel,
            |_| {},
        )
        .await;
        if let Err(e) = ran {
            // A half-made environment would look ready next time.
            let _ = std::fs::remove_dir_all(project.join(".venv"));
            return Err(e);
        }
    }
    let dir = ditto::models_dir(paths);
    let missing: Vec<&train::TrainerFile> = d.files.iter().filter(|f| !dir.join(&f.file).is_file()).collect();
    if !missing.is_empty() {
        state.log("network", "Downloading Ditto's model files from huggingface.co");
        let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false)?;
        let total: u64 = missing.iter().map(|f| f.size).sum::<u64>().max(1);
        let mut done = 0u64;
        for f in missing {
            let base = done;
            download::fetch_verified(&client, &f.url, &dir.join(&f.file), f.size, &f.sha256, ctx.cancel, |r, _| (ctx.progress)(say, 0.03 + 0.06 * (base + r) as f64 / total as f64)).await?;
            done += f.size;
        }
    }
    Ok(uv)
}

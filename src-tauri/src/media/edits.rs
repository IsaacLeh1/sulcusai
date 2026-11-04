// SPDX-License-Identifier: AGPL-3.0-only
//! Videos edited, given sound, and dubbed into other languages and voices.

use super::*;

/// What to do to a video (`video_edit`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct VideoEdit {
    /// Keep only this stretch of it (seconds from its start).
    pub trim: Option<[f64; 2]>,
    /// Other clips (gallery ids) joined after it, in order.
    pub join: Vec<String>,
    /// A sound track: a song or sound from the gallery, or a file.
    pub audio: Option<String>,
    pub audio_file: Option<String>,
    /// "replace" its sound (the default) or "mix" with it.
    pub audio_mode: Option<String>,
    /// The sound track's loudness (1 as it is).
    pub volume: Option<f32>,
    /// Its sound taken off.
    pub mute: bool,
    /// Edited by transcript: only these stretches kept, joined in order.
    pub keep: Vec<[f64; 2]>,
    /// Its speech cleaned (noise, hum, level): "gentle" or "strong".
    pub clean_speech: Option<String>,
}

/// A gallery item written out for FFmpeg.
pub(super) fn written_out(state: &AppState, c: &crate::crypto::Cipher, item: &MediaItem, to: &std::path::Path) -> Result<PathBuf, String> {
    let ext = match item.mime.as_str() {
        m if m.contains("webm") => "webm",
        m if m.contains("mp4") => "mp4",
        m if m.contains("wav") => "wav",
        m if m.contains("mpeg") => "mp3",
        m if m.contains("ogg") => "ogg",
        _ => if item.kind == "video" { "mp4" } else { "wav" },
    };
    let path = to.with_extension(ext);
    std::fs::write(&path, store::read(&state.paths, c, &item.id)?).map_err(|e| e.to_string())?;
    Ok(path)
}

pub(super) async fn video_edit(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let edit = ctx.req.edit.clone().unwrap_or_default();
    Ok(vec![edit_video(ctx, work, &id, &edit, "video_edit").await?])
}

/// A video with `edit` done to it, kept as a new video beside it.
pub(super) async fn edit_video(ctx: &RunCtx<'_>, work: &std::path::Path, id: &str, edit: &VideoEdit, op: &str) -> Result<MediaItem, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That video is no longer in the gallery.")?;
    if item.kind != "video" {
        return Err("Pick a video.".into());
    }
    let (ffmpeg, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Getting the clips ready", 0.05);
    let mut clips = vec![written_out(state, &c, &item, &work.join("clip0"))?];
    if !edit.keep.is_empty() || edit.clean_speech.is_some() {
        clips[0] = prepass(ctx, &ffmpeg, &ffprobe, &clips[0], edit, work).await?;
    }
    for (i, other) in edit.join.iter().enumerate() {
        let o = store::get(&state.db.lock().unwrap(), &c, other).ok_or("A clip to join is no longer in the gallery.")?;
        if o.kind != "video" {
            return Err("Only videos can be joined.".into());
        }
        clips.push(written_out(state, &c, &o, &work.join(format!("clip{}", i + 1)))?);
    }
    let mut probes = Vec::new();
    for p in &clips {
        probes.push(ffmpeg::probe(&ffprobe, p).await?);
    }
    // The first clip cut to its stretch.
    let trim = edit.trim.filter(|[a, b]| b > a).map(|[a, b]| [a.max(0.0), b.min(probes[0].seconds.max(b))]);
    if let Some([a, b]) = trim {
        probes[0].seconds = (b - a).max(0.1);
    }
    let total: f64 = probes.iter().map(|p| p.seconds).sum();
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    for (i, p) in clips.iter().enumerate() {
        if let (0, Some([a, b])) = (i, trim) {
            args.extend([format!("-ss"), format!("{a:.3}"), format!("-to"), format!("{b:.3}")].map(Into::into));
        }
        args.extend(["-i".into(), p.as_os_str().to_owned()]);
    }
    // A sound track, after the clips.
    let track = match (&edit.audio, &edit.audio_file) {
        (Some(aid), _) => {
            let a = store::get(&state.db.lock().unwrap(), &c, aid).ok_or("That sound is no longer in the gallery.")?;
            Some(written_out(state, &c, &a, &work.join("track"))?)
        }
        (None, Some(f)) => Some(PathBuf::from(f)),
        _ => None,
    };
    let mut filter = ffmpeg::join_filter(&probes);
    let audio_out = match &track {
        Some(t) => {
            args.extend(["-i".into(), t.as_os_str().to_owned()]);
            let k = clips.len();
            let vol = edit.volume.unwrap_or(1.0).clamp(0.0, 2.0);
            if edit.audio_mode.as_deref() == Some("mix") && !edit.mute {
                filter.push_str(&format!(";[{k}:a]aresample=44100,aformat=channel_layouts=stereo,volume={vol:.2}[t];[a][t]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[aout]"));
            } else {
                // The clips' own sound goes unused (FFmpeg refuses an output left unconnected).
                filter.push_str(&format!(";[a]anullsink;[{k}:a]aresample=44100,aformat=channel_layouts=stereo,volume={vol:.2},apad,atrim=duration={total:.3}[aout]"));
            }
            Some("[aout]")
        }
        None if edit.mute => {
            filter.push_str(";[a]anullsink");
            None
        }
        None => Some("[a]"),
    };
    let out = work.join("edited.mp4");
    args.extend(["-filter_complex".into(), filter.into(), "-map".into(), "[v]".into()]);
    match audio_out {
        Some(a) => args.extend(["-map".into(), a.into()]),
        None => args.push("-an".into()),
    }
    args.extend(["-t".into(), format!("{total:.3}").into()]);
    args.extend(ffmpeg::mp4_out(&out));
    let stage = if op == "dub" { "Putting the new voice in" } else { "Making the video" };
    (ctx.progress)(stage, 0.1);
    ffmpeg::run(state, &ffmpeg, args, total, ctx.cancel, &|f| (ctx.progress)(stage, 0.1 + 0.85 * f)).await?;
    // A still from it for the gallery.
    let still = work.join("still.jpg");
    let _ = ffmpeg::run(state, &ffmpeg, ["-ss", &format!("{:.2}", (total / 3.0).min(2.0)), "-i"].iter().map(Into::into).chain([out.as_os_str().to_owned(), "-frames:v".into(), "1".into(), "-q:v".into(), "3".into(), still.as_os_str().to_owned()]).collect(), 0.0, ctx.cancel, &|_| {}).await;
    let thumb = std::fs::read(&still).ok().and_then(|b| imaging::decode(&b).ok()).and_then(|img| imaging::thumbnail(&img).ok());
    let bytes = std::fs::read(&out).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let made = MediaItem {
        kind: "video".into(),
        op: op.into(),
        prompt: item.prompt.clone(),
        model_id: item.model_id.clone(),
        mime: "video/mp4".into(),
        seconds: total,
        width: probes[0].width,
        height: probes[0].height,
        parent: Some(item.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| item.chat_id.clone()),
        poster: thumb.is_some(),
        ..Default::default()
    };
    store::save(&state.db.lock().unwrap(), &state.paths, &c, made, &bytes, thumb.as_deref())
}

/// The clip cut to the stretches kept from its transcript and/or with its
/// speech cleaned, before the rest of the edit.
async fn prepass(ctx: &RunCtx<'_>, ffmpeg_exe: &std::path::Path, ffprobe: &std::path::Path, clip: &std::path::Path, edit: &VideoEdit, work: &std::path::Path) -> Result<PathBuf, String> {
    let probe = ffmpeg::probe(ffprobe, clip).await?;
    let clean = match (&edit.clean_speech, probe.audio) {
        (Some(how), true) => {
            (ctx.progress)("Listening to the speech", 0.02);
            let st = ffmpeg_stereo(ctx.state, ffmpeg_exe, clip, probe.seconds, ctx.cancel).await?;
            let mono: Vec<f32> = st.ch[0].iter().zip(&st.ch[1]).map(|(a, b)| (a + b) / 2.0).collect();
            let check = super::create::mic::check(&mono, st.rate);
            Some(super::create::mic::clean_filter(&check, how == "strong"))
        }
        _ => None,
    };
    let out = work.join("prepass.mp4");
    let Some(args) = super::create::run::prepass_args(clip, &out, &edit.keep, clean.as_deref(), probe.audio) else {
        return Ok(clip.to_path_buf());
    };
    (ctx.progress)(if edit.keep.is_empty() { "Cleaning the speech" } else { "Cutting by the transcript" }, 0.04);
    ffmpeg::run(ctx.state, ffmpeg_exe, args, probe.seconds, ctx.cancel, &|_| {}).await?;
    Ok(out)
}

/// A sound made for a video by the music model (a sound effect for a short
/// clip, instrumental music for a longer one), from its description or the
/// one given, as long as the video; then put on it (mixed with its sound
/// when it has some).
pub(super) async fn video_sound(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let (_, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let probe = ffmpeg::probe(&ffprobe, &written_out(state, &c, &item, &work.join("look"))?).await?;
    let secs = probe.seconds.max(item.seconds).max(1.0);
    let op = if secs <= 30.0 { "sound" } else { "music" };
    let model = choose(state, op, None, false).map_err(|_| "Making a sound for a video needs a music model (Studio › Models).".to_string())?;
    let described = if ctx.req.prompt.trim().is_empty() { format!("the sound of this scene: {}", item.prompt) } else { ctx.req.prompt.trim().to_string() };
    let ask = Request { op: op.into(), prompt: described, seconds: Some(secs.ceil().max(10.0) as f32), instrumental: Some(true), chat_id: ctx.req.chat_id.clone(), ..Default::default() };
    let stage = |s: &str, f: f64| (ctx.progress)(s, 0.6 * f);
    let step = RunCtx { state, req: &ask, cancel: ctx.cancel, progress: &stage, reserved: ctx.reserved };
    let made = Box::pin(song(&step, &model, work)).await?;
    let sound = made.into_iter().find(|i| i.kind == "audio").ok_or("The music model didn't make a sound.")?;
    let edit = VideoEdit { audio: Some(sound.id.clone()), audio_mode: Some(if probe.audio { "mix".into() } else { "replace".into() }), volume: Some(if probe.audio { 0.7 } else { 1.0 }), ..Default::default() };
    let progress = |s: &str, f: f64| (ctx.progress)(s, 0.6 + 0.4 * f);
    let rest = RunCtx { state, req: ctx.req, cancel: ctx.cancel, progress: &progress, reserved: ctx.reserved };
    Ok(vec![edit_video(&rest, work, &id, &edit, "video_sound").await?])
}

/// What's said in a video and when (seconds), for dubbing: whisper's pieces
/// joined into sentences, none past 12 s, so the new voice keeps in time.
pub(super) fn dub_lines(segments: Vec<crate::speech::Segment>) -> Vec<(f64, f64, String)> {
    let mut out: Vec<(f64, f64, String)> = Vec::new();
    for s in segments {
        let text = s.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if let Some(prev) = out.last_mut() {
            let ended = prev.2.ends_with(['.', '!', '?', '…', '。', '？', '！']);
            if !ended && s.start - prev.1 < 0.6 && s.end - prev.0 <= 12.0 {
                prev.2.push(' ');
                prev.2.push_str(&text);
                prev.1 = s.end;
                continue;
            }
        }
        out.push((s.start, s.end, text));
    }
    out
}

/// How much faster a line `said` seconds long goes to fit in `room`: 1 when
/// it fits, at most 1.5 (past that the words blur).
pub(super) fn dub_speed(said: f64, room: f64) -> f64 {
    if room <= 0.0 || said <= room * 1.03 {
        1.0
    } else {
        (said / room).min(1.5)
    }
}

/// The dub's voice choice that gives everyone in the video their own voice.
pub(super) const EACH_VOICE: &str = "each";

/// Who says each line (1, 2, …) by the sound of their voice (`voice16`, the
/// speech at 16 kHz): the speaker model, got the first time (~30 MB);
/// everyone as one without it. Lines too short to judge go with the nearest
/// line that isn't, and someone heard for under 3 s with whoever sounds most
/// like them (too little to make a voice from).
pub(super) async fn dub_speakers(ctx: &RunCtx<'_>, voice16: &[f32], lines: &[(f64, f64, String)]) -> Vec<u32> {
    let state = ctx.state;
    if lines.len() < 2 {
        return vec![1; lines.len()];
    }
    if !crate::diarize::available(state) {
        if let Err(e) = crate::diarize::install(state, ctx.cancel, &|_, _, _| (ctx.progress)("Getting the speaker model (once)", 0.55)).await {
            state.log("media", &format!("Dubbing: the speaker model didn't install ({e}); everyone gets one voice"));
            return vec![1; lines.len()];
        }
    }
    let rate = crate::audio::RATE as f64;
    let prints: Vec<Option<Vec<f32>>> = tokio::task::block_in_place(|| {
        lines
            .iter()
            .map(|(s, e, _)| {
                let (a, b) = (((s * rate) as usize).min(voice16.len()), ((e * rate) as usize).min(voice16.len()));
                (e - s >= crate::diarize::MIN_SECS).then(|| crate::diarize::embed(state, &voice16[a..b]).ok()).flatten()
            })
            .collect()
    });
    let known: Vec<usize> = (0..lines.len()).filter(|i| prints[*i].is_some()).collect();
    if known.is_empty() {
        return vec![1; lines.len()];
    }
    let grouped = crate::diarize::regroup(&known.iter().filter_map(|i| prints[*i].clone()).collect::<Vec<_>>());
    let mut labels: Vec<u32> = (0..lines.len())
        .map(|i| {
            let near = (0..known.len()).min_by(|a, b| (lines[known[*a]].0 - lines[i].0).abs().total_cmp(&(lines[known[*b]].0 - lines[i].0).abs())).unwrap_or(0);
            grouped[near]
        })
        .collect();
    // Each speaker's voice on average, and how long they speak.
    let speakers = grouped.iter().copied().max().unwrap_or(1);
    let mean = |k: u32| -> Vec<f32> {
        let mut m = vec![0f32; prints[known[0]].as_ref().map_or(0, Vec::len)];
        for (i, _) in known.iter().zip(&grouped).filter(|(_, g)| **g == k) {
            for (x, y) in m.iter_mut().zip(prints[*i].iter().flatten()) {
                *x += y;
            }
        }
        let n = m.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        m.into_iter().map(|x| x / n).collect()
    };
    let heard = |k: u32, labels: &[u32]| lines.iter().zip(labels).filter(|(_, l)| **l == k).map(|(l, _)| l.1 - l.0).sum::<f64>();
    let means: Vec<Vec<f32>> = (1..=speakers).map(mean).collect();
    for k in 1..=speakers {
        if speakers > 1 && heard(k, &labels) < 3.0 {
            let like = (1..=speakers).filter(|o| *o != k && heard(*o, &labels) >= 3.0).max_by(|a, b| crate::diarize::cosine(&means[k as usize - 1], &means[*a as usize - 1]).total_cmp(&crate::diarize::cosine(&means[k as usize - 1], &means[*b as usize - 1])));
            if let Some(o) = like {
                labels.iter_mut().filter(|l| **l == k).for_each(|l| *l = o);
            }
        }
    }
    labels
}

/// The dub's lines read in each speaker's own voice, made from how they
/// sound in the video: their longest lines (~10 to 15 s together, with the
/// words heard in them) taken from the speech alone (the video's sound less
/// its music and other sounds, when the separator took those out).
pub(super) async fn each_own_voice(ctx: &RunCtx<'_>, mix: &stems::Stereo, background: Option<&[Vec<f32>; 2]>, lines: &[(f64, f64, String)], words: &[String], to: &str) -> Result<(Vec<Vec<f32>>, u32), String> {
    let state = ctx.state;
    let speech: Vec<f32> = match background {
        Some([l, r]) => mix.ch[0].iter().zip(&mix.ch[1]).zip(l.iter().zip(r)).map(|((a, b), (c, d))| (a + b - c - d) * 0.5).collect(),
        None => mix.mono(),
    };
    let labels = dub_speakers(ctx, &stems::resample(&speech, stems::RATE, crate::audio::RATE), lines).await;
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let at24 = stems::resample(&speech, stems::RATE, own_voice::TTS_RATE);
    let rate = own_voice::TTS_RATE as f64;
    let speakers = labels.iter().copied().max().unwrap_or(1);
    let who: Vec<u32> = (1..=speakers).filter(|k| labels.contains(k)).collect();
    let mut out = vec![Vec::new(); lines.len()];
    for (n, k) in who.iter().enumerate() {
        let theirs: Vec<usize> = (0..lines.len()).filter(|i| labels[*i] == *k).collect();
        // Their longest lines, in the order said.
        let mut longest = theirs.clone();
        longest.sort_by(|a, b| (lines[*b].1 - lines[*b].0).total_cmp(&(lines[*a].1 - lines[*a].0)));
        let mut chosen: Vec<usize> = Vec::new();
        let mut secs = 0.0;
        for i in longest {
            let d = lines[i].1 - lines[i].0;
            if secs >= 10.0 || (!chosen.is_empty() && secs + d > 15.0) {
                continue;
            }
            chosen.push(i);
            secs += d;
        }
        chosen.sort_unstable();
        let gap = vec![0f32; (rate * 0.25) as usize];
        let mut reference = Vec::new();
        for i in &chosen {
            let (a, b) = (((lines[*i].0 * rate) as usize).min(at24.len()), ((lines[*i].1 * rate) as usize).min(at24.len()));
            if !reference.is_empty() {
                reference.extend_from_slice(&gap);
            }
            reference.extend_from_slice(&at24[a..b]);
        }
        let level = crate::audio::rms(&reference);
        if level > 1e-4 {
            reference.iter_mut().for_each(|x| *x = (*x * 0.08 / level).clamp(-1.0, 1.0));
        }
        let said: String = chosen.iter().map(|i| lines[*i].2.as_str()).collect::<Vec<_>>().join(" ");
        let passages: Vec<String> = theirs.iter().map(|i| words[*i].clone()).collect();
        let (from, span) = (0.55 + 0.25 * n as f64 / who.len() as f64, 0.25 / who.len() as f64);
        let label = if who.len() > 1 { format!("Speaking it in voice {} of {}", n + 1, who.len()) } else { "Speaking it in their own voice".to_string() };
        let (read, _) = own_voice::speak_each_like(state, &reference, &said, &passages, Some(to), ctx.cancel, &|_, f| (ctx.progress)(&label, from + span * f)).await?;
        for (i, r) in theirs.iter().zip(read) {
            out[*i] = r;
        }
    }
    state.log("media", &format!("Dubbing in each speaker's own voice: {} speaker(s) heard", who.len()));
    Ok((out, own_voice::TTS_RATE))
}

/// A sound file as stereo at the separator's rate, read by FFmpeg.
pub(super) async fn ffmpeg_stereo(state: &AppState, ffmpeg_exe: &std::path::Path, file: &std::path::Path, seconds: f64, cancel: &AtomicBool) -> Result<stems::Stereo, String> {
    let raw = file.with_extension("f32");
    let rate = stems::RATE.to_string();
    let args: Vec<std::ffi::OsString> = ["-i".into(), file.as_os_str().to_owned()].into_iter().chain(["-vn", "-ac", "2", "-ar", &rate, "-f", "f32le"].map(Into::into)).chain([raw.as_os_str().to_owned()]).collect();
    ffmpeg::run(state, ffmpeg_exe, args, seconds, cancel, &|_| {}).await?;
    let bytes = std::fs::read(&raw).map_err(|e| format!("FFmpeg didn't read the sound: {e}"))?;
    let _ = std::fs::remove_file(&raw);
    let all: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    Ok(stems::Stereo { rate: stems::RATE, ch: [all.iter().step_by(2).copied().collect(), all.iter().skip(1).step_by(2).copied().collect()] })
}

/// Samples at the separator's rate, `speed` times faster at the same pitch.
pub(super) async fn faster(state: &AppState, ffmpeg_exe: &std::path::Path, x: &[f32], speed: f64, at: &std::path::Path, cancel: &AtomicBool) -> Result<Vec<f32>, String> {
    let (inp, out) = (at.with_extension("in.wav"), at.with_extension("out.wav"));
    std::fs::write(&inp, crate::audio::wav_encode(x, stems::RATE)).map_err(|e| e.to_string())?;
    let args: Vec<std::ffi::OsString> = ["-i".into(), inp.as_os_str().to_owned(), "-filter:a".into(), format!("atempo={speed:.3}").into(), out.as_os_str().to_owned()].into();
    ffmpeg::run(state, ffmpeg_exe, args, 0.0, cancel, &|_| {}).await?;
    Ok(ffmpeg_stereo(state, ffmpeg_exe, &out, 0.0, cancel).await?.mono())
}

/// A video's speech heard, translated and said again in another language,
/// in time with it: each line in its own stretch (a little faster when the
/// new words run longer), over the video's music and other sounds (the
/// part separator takes the old voice out) or, without the separator, over
/// its sound turned low while someone speaks. Read by the voice picked (a
/// recorded one is "own:<id>") or one that speaks the language.
pub(super) async fn dub(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let to = ctx.req.language.clone().filter(|l| !l.is_empty() && l != "auto").ok_or("Pick the language to dub it into.")?;
    let into = crate::translate::language_name(&to);
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let video = written_out(state, &c, &item, &work.join("source"))?;
    let probe = ffmpeg::probe(&ffprobe, &video).await?;
    if !probe.audio {
        return Err("This video has no sound, so there's nothing to dub.".into());
    }
    (ctx.progress)("Listening to what's said", 0.05);
    let mix = ffmpeg_stereo(state, &ffmpeg_exe, &video, probe.seconds, ctx.cancel).await?;
    let speech = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.map_err(|e| format!("Dubbing needs a speech model to hear what's said. {e}"))?;
    let options = crate::speech::Options { language: Some("auto".into()), ..Default::default() };
    let heard = crate::speech::transcribe(&speech, &stems::resample(&mix.mono(), stems::RATE, crate::audio::RATE), &options).await?;
    let lines = dub_lines(crate::speech::clean(heard).segments);
    if lines.is_empty() {
        return Err("No one speaks in this video, so there's nothing to dub.".into());
    }
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }

    (ctx.progress)("Translating what's said", 0.2);
    let said: Vec<String> = lines.iter().map(|l| l.2.clone()).collect();
    let words = {
        let (ep, _, _hold) = crate::background_endpoint(state).await.map_err(|e| format!("Translating what's said needs a chat model: {e}"))?;
        crate::translate::translate_lines(&ep, &said, &to).await?
    };

    // Its music and other sounds without the voice.
    (ctx.progress)("Taking the voice out", 0.35);
    let background = separated(ctx, &mix, &[0, 1, 2], "the old sound is turned down under the new voice instead").await?;

    let speaking = format!("Speaking it in {into}");
    (ctx.progress)(&speaking, 0.55);
    let picked = ctx.req.voice.as_deref().filter(|v| *v != EACH_VOICE);
    let mut cloned = None;
    if ctx.req.voice.as_deref() == Some(EACH_VOICE) {
        if !own_voice::speaks(&to) {
            state.log("media", &format!("Dubbing: voices made from the speakers don't speak {into}; using a voice that does"));
        } else {
            match each_own_voice(ctx, &mix, background.as_ref(), &lines, &words, &to).await {
                Ok(v) => cloned = Some(v),
                Err(e) if e == download::CANCELLED => return Err(e),
                Err(e) => state.log("media", &format!("Dubbing in each speaker's own voice didn't work ({e}); using a voice that speaks {into}")),
            }
        }
    }
    let (spoken, voice_rate) = match (cloned, picked.and_then(|v| v.strip_prefix("own:"))) {
        (Some(v), _) => v,
        (None, Some(own)) => own_voice::speak_each(state, own, &words, Some(&to), ctx.cancel, &|s, f| (ctx.progress)(s, 0.55 + 0.25 * f)).await?,
        (None, None) => tokio::task::block_in_place(|| -> Result<(Vec<Vec<f32>>, u32), String> {
            let voices = crate::tts::voices()?;
            let voice = crate::tts::pick_voice(&voices, picked, Some(&to)).map(|v| v.id.clone()).ok_or("No voices are installed.")?;
            let (mut out, mut rate) = (Vec::new(), 16_000);
            for (i, w) in words.iter().enumerate() {
                if ctx.cancel.load(Ordering::Relaxed) {
                    return Err(download::CANCELLED.into());
                }
                (ctx.progress)(&speaking, 0.55 + 0.25 * i as f64 / words.len() as f64);
                let s = crate::tts::synthesize(&crate::tts::speakable(w), Some(&voice), Some(&to), 1.0)?;
                rate = s.rate;
                out.push(s.samples.iter().map(|x| *x as f32 / 32768.0).collect());
            }
            Ok((out, rate))
        })?,
    };

    // Each line at its time, as loud as speech usually is.
    (ctx.progress)("Putting the new voice in", 0.8);
    let sr = stems::RATE as f64;
    let total = mix.len();
    let ducked = background.is_none();
    let [mut l, mut r] = background.unwrap_or_else(|| mix.ch.clone());
    let mut gain = vec![1.0f32; if ducked { total } else { 0 }];
    let mut voice = vec![0.0f32; total];
    for (i, ((start, end, _), said)) in lines.iter().zip(&spoken).enumerate() {
        let next = lines.get(i + 1).map_or(probe.seconds.max(*end), |n| n.0);
        let room = (next - start).max(end - start).max(0.3);
        let mut v = stems::resample(said, voice_rate, stems::RATE);
        let speed = dub_speed(v.len() as f64 / sr, room);
        if speed > 1.0 {
            v = faster(state, &ffmpeg_exe, &v, speed, &work.join(format!("line{i}")), ctx.cancel).await?;
        }
        let rms = (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt();
        let level = if rms > 1e-4 { 0.1 / rms } else { 1.0 };
        let at = (start * sr) as usize;
        for (k, x) in v.iter().enumerate() {
            if let Some(s) = voice.get_mut(at + k) {
                *s += x * level;
            }
        }
        if ducked {
            // The old sound turned down under the line, with short fades.
            let fade = (0.15 * sr) as usize;
            for s in at.saturating_sub(fade)..(at + v.len() + fade).min(total) {
                let away = if s < at { (at - s) as f32 } else { s.saturating_sub(at + v.len()) as f32 };
                gain[s] = gain[s].min(0.2 + 0.8 * (away / fade as f32).min(1.0));
            }
        }
    }
    for s in 0..total {
        let g = gain.get(s).copied().unwrap_or(1.0);
        l[s] = l[s] * g + voice[s];
        r[s] = r[s] * g + voice[s];
    }
    let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
    if peak > 0.98 {
        let k = 0.98 / peak;
        l.iter_mut().chain(r.iter_mut()).for_each(|x| *x *= k);
    }
    let track = work.join("dubbed.wav");
    std::fs::write(&track, own_voice::wav_stereo(&l, &r, stems::RATE)).map_err(|e| e.to_string())?;
    let edit = VideoEdit { audio_file: Some(track.to_string_lossy().into_owned()), audio_mode: Some("replace".into()), volume: Some(1.0), ..Default::default() };
    let progress = |s: &str, f: f64| (ctx.progress)(s, 0.85 + 0.15 * f);
    let rest = RunCtx { state, req: ctx.req, cancel: ctx.cancel, progress: &progress, reserved: ctx.reserved };
    let made = edit_video(&rest, work, &id, &edit, "dub").await?;
    state.log("media", &format!("Dubbed a video into {into} on this PC ({} lines{})", lines.len(), if ducked { ", the old sound turned down" } else { "" }));
    Ok(vec![made])
}

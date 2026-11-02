// SPDX-License-Identifier: AGPL-3.0-only
//! Narration in a recorded or designed voice (Qwen3-TTS through
//! qwentts.cpp), and voices designed from a description.

use super::*;

/// Qwen3-TTS's name for a language (None: it works it out).
pub(super) fn tts_language(code: Option<&str>) -> &'static str {
    match code.map(|c| c.split(['-', '_']).next().unwrap_or("").to_lowercase()).as_deref() {
        Some("en") => "English",
        Some("zh") => "Chinese",
        Some("ja") => "Japanese",
        Some("ko") => "Korean",
        Some("de") => "German",
        Some("fr") => "French",
        Some("ru") => "Russian",
        Some("pt") => "Portuguese",
        Some("es") => "Spanish",
        Some("it") => "Italian",
        _ => "auto",
    }
}

/// Text in lines of whole sentences, each at most ~300 characters: the
/// engine speaks each line on its own, with the person's voice each time.
pub(super) fn lines_of(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for paragraph in text.split('\n').map(str::trim).filter(|p| !p.is_empty()) {
        let mut sentence = String::new();
        let flush = |s: &mut String, cur: &mut String, out: &mut Vec<String>| {
            let s2 = s.trim().to_string();
            s.clear();
            if s2.is_empty() {
                return;
            }
            if !cur.is_empty() && cur.len() + s2.len() > 300 {
                out.push(std::mem::take(cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(&s2);
        };
        for ch in paragraph.chars() {
            sentence.push(ch);
            if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | ';') {
                flush(&mut sentence, &mut cur, &mut out);
            }
        }
        flush(&mut sentence, &mut cur, &mut out);
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    out
}

/// Each line's WAV from the engine's output (`--stream-by-line -o -`): one
/// header per line, each 44 bytes with no length (0x7fffffff), 16-bit mono.
pub(super) fn split_streamed(bytes: &[u8]) -> Vec<Vec<f32>> {
    const MARK: &[u8] = b"RIFF\xff\xff\xff\x7fWAVE";
    let mut starts: Vec<usize> = bytes.windows(MARK.len()).enumerate().filter(|(_, w)| *w == MARK).map(|(i, _)| i).collect();
    starts.push(bytes.len());
    starts
        .windows(2)
        .map(|w| bytes[(w[0] + 44).min(w[1])..w[1]].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect())
        .collect()
}

/// The narration engine, downloaded the first time: the graphics-card build
/// where there's a card.
pub(super) async fn voice_engine(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<PathBuf, String> {
    let spec = state.catalog.media.own_voice.as_ref().map(|c| c.speak.clone()).ok_or("Speaking in your voice isn't available on this system yet.")?;
    let gpu = state.budget().vram > 0 && cfg!(all(windows, target_arch = "x86_64"));
    let getting = "Getting the voice engine (once)";
    if let Some(asset) = spec.engine.assets.get("vulkan-x64").filter(|_| gpu) {
        let dir = format!("qwentts-{}-vulkan-x64", spec.engine.build);
        match engine::ensure_unpacked(&state.paths, &dir, asset, TTS_EXE, cancel, |r, t| progress(getting, r as f64 / t.max(1) as f64)).await {
            Ok(p) => return Ok(p),
            Err(e) if e == download::CANCELLED => return Err(e),
            Err(e) => state.log("media", &format!("The graphics-card voice engine didn't download ({e}); using the processor one")),
        }
    }
    let asset = spec.engine.assets.get(engine::cpu_key()).ok_or("Speaking in your voice isn't available on this system yet.")?;
    let dir = format!("qwentts-{}-{}", spec.engine.build, engine::cpu_key());
    engine::ensure_unpacked(&state.paths, &dir, asset, TTS_EXE, cancel, |r, t| progress(getting, r as f64 / t.max(1) as f64)).await
}

/// The narration engine and model, downloaded the first time: the
/// graphics-card build and the bigger model where there's a card.
pub(super) async fn speak_parts(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let spec = state.catalog.media.own_voice.as_ref().map(|c| c.speak.clone()).ok_or("Speaking in your voice isn't available on this system yet.")?;
    let exe = voice_engine(state, cancel, progress).await?;
    let on_card = exe.to_string_lossy().contains("vulkan");
    let files = if on_card { &spec.files } else { &spec.processor_files };
    Ok((exe, fetch_all(state, files, "own-voice", "Getting the voice model (once", cancel, progress).await?))
}

/// Whether reading in a recorded voice can start without downloading
/// anything (Check this PC tries it only then).
pub fn ready_here(state: &AppState) -> bool {
    let Some(spec) = state.catalog.media.own_voice.as_ref().map(|c| &c.speak) else { return false };
    let base = state.paths.models.join("media").join("own-voice");
    let has = |files: &[MediaFile]| files.iter().all(|f| base.join(&f.file).metadata().is_ok_and(|m| m.len() == f.size));
    let engine_here = ["vulkan-x64", engine::cpu_key()].iter().any(|k| state.paths.engines.join(format!("qwentts-{}-{}", spec.engine.build, k)).is_dir());
    engine_here && (has(&spec.files) || has(&spec.processor_files))
}

/// The engine and the model that makes voices from words, downloaded the
/// first time.
pub(super) async fn design_parts(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let spec = state.catalog.media.own_voice.as_ref().and_then(|c| c.design.clone()).ok_or("Making a voice from words isn't available on this system yet.")?;
    let exe = voice_engine(state, cancel, progress).await?;
    Ok((exe, fetch_all(state, &spec.files, "own-voice", "Getting the voice maker (once", cancel, progress).await?))
}

/// Model files into models/media/<dir>, downloaded the first time.
pub(super) async fn fetch_all(state: &Arc<AppState>, files: &[MediaFile], dir: &str, getting: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<PathBuf>, String> {
    let base = state.paths.models.join("media").join(dir);
    let missing: u64 = files.iter().filter(|f| !base.join(&f.file).metadata().is_ok_and(|m| m.len() == f.size)).map(|f| f.size).sum();
    let label = format!("{getting}, {})", crate::size_label(missing));
    let mut done = 0u64;
    let mut out = Vec::new();
    for f in files {
        let dest = base.join(&f.file);
        if !dest.metadata().is_ok_and(|m| m.len() == f.size) {
            state.log("network", &format!("Downloading {} ({}) from {}", f.file, crate::size_label(f.size), crate::host_of(&f.url)));
            let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
            let before = done;
            download::fetch_verified(&client, &f.url, &dest, f.size, &f.sha256, cancel, |r, _| progress(&label, (before + r) as f64 / missing.max(1) as f64)).await?;
            done += f.size;
        }
        out.push(dest);
    }
    Ok(out)
}

/// Reads `text` aloud in a recorded voice: 24 kHz samples.
pub async fn speak(state: &Arc<AppState>, voice_id: &str, text: &str, language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(Vec<f32>, u32), String> {
    let c = state.work_cipher()?;
    hear_missing_words(state, &c, voice_id, &|_| progress("Hearing the words of your recordings (once)", 0.05)).await;
    let voice = get(&state.db.lock().unwrap(), &c, voice_id).ok_or("That voice is no longer here.")?;
    let (reference, words) = speech_reference(&state.paths, &c, &voice)?;
    let lines = lines_of(text);
    if lines.is_empty() {
        return Err("Type the text to read aloud.".into());
    }
    let (exe, files) = speak_parts(state, cancel, progress).await?;
    progress("Reading in your voice", 0.1);
    // A card that's nearly full (a game, a big model) makes the engine crawl
    // (65 s for a sentence): the same engine and model then run on the
    // processor (~10 s).
    let free = card_free_now(state);
    let device = (exe.to_string_lossy().contains("vulkan") && free.is_some_and(|f| f < 3_500 << 20)).then_some("CPU");
    if device.is_some() {
        crate::applog::line("media", &format!("Reading in a recorded voice on the processor: the graphics card has {} free", crate::size_label(free.unwrap_or(0))));
    }
    read_with(&exe, &files, &reference, &words, &lines, language, device, cancel).await
}

/// What the graphics card has free now, from Windows (None when it can't
/// tell).
pub(super) fn card_free_now(state: &AppState) -> Option<u64> {
    let total = state.hardware.read().unwrap().best_gpu().map_or(0, |g| g.vram_bytes);
    let used = crate::perf::card_memory_used(crate::hardware::dedicated().and_then(|c| c.luid))?;
    (total > 0).then(|| total.saturating_sub(used))
}

/// The longest a line may take read slowly (8 characters a second), and 3 s
/// more.
pub(super) fn longest_s(line: &str) -> f32 {
    line.chars().count() as f32 / 8.0 + 3.0
}

/// Whose voice the engine reads in.
#[derive(Clone, Copy)]
pub(super) enum How<'a> {
    /// The reference in `work` (`ref.wav`; with `with_words`, also `ref.txt`,
    /// its words: in-context cloning, which keeps the way the person speaks).
    Clone { work: &'a Path, with_words: bool },
    /// A voice made from this description (the VoiceDesign model).
    Design(&'a str),
}

/// The engine making `lines` in a voice: one reading per line. Each is
/// capped near its line's length (the engine's own cap is ~170 s).
pub(super) async fn run_engine(exe: &Path, files: &[PathBuf], how: How<'_>, lines: &[String], language: Option<&str>, device: Option<&str>, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    // Qwen3-TTS makes 12.5 frames a second.
    let max_new = lines.iter().map(|l| longest_s(l)).fold(0f32, f32::max) * 12.5;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.current_dir(exe.parent().unwrap_or(Path::new("."))).arg("--model").arg(&files[0]).arg("--codec").arg(&files[1]);
    match how {
        How::Clone { work, with_words } => {
            cmd.arg("--ref-wav").arg(work.join("ref.wav"));
            if with_words {
                cmd.arg("--ref-text").arg(work.join("ref.txt"));
            }
        }
        How::Design(description) => {
            cmd.arg("--instruct").arg(description);
        }
    }
    if let Some(d) = device {
        cmd.args(["--device", d]);
    }
    cmd.args(["--lang", tts_language(language), "--max-new", &(max_new.ceil() as u32).to_string(), "--stream-by-line", "-o", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| format!("The voice engine didn't start: {e}"))?;
    let input = lines.join("\n") + "\n";
    let mut stdin = child.stdin.take().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(input.as_bytes()).await;
    });
    let out = child.wait_with_output();
    tokio::pin!(out);
    let out = loop {
        tokio::select! {
            o = &mut out => break o.map_err(|e| e.to_string())?,
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(download::CANCELLED.to_string());
                }
            }
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        crate::applog::line("media", &format!("The voice engine failed: {}", err.lines().rev().take(4).collect::<Vec<_>>().join(" | ")));
        return Err("The voice engine stopped with an error.".into());
    }
    Ok(split_streamed(&out.stdout))
}

/// Whether any reading came out as sound (not silence all through, nor
/// samples that aren't numbers).
pub(super) fn heard_any(parts: &[Vec<f32>]) -> bool {
    parts.iter().any(|p| !p.is_empty() && p.iter().all(|s| s.is_finite()) && crate::audio::rms(p) > 0.003)
}

/// `lines` read in the voice of `reference` (24 kHz) whose words are
/// `words`. A line that runs on far past its length (the engine lost its
/// end, as when the words don't match the recording) is read again from the
/// voice alone, without the words. Readings that come out silent on the
/// graphics card (some cards' drivers give silence or broken samples
/// instead of failing) are read again on the processor; silence there too
/// is an error, never saved as a reading.
#[allow(clippy::too_many_arguments)]
pub(super) async fn read_with(exe: &Path, files: &[PathBuf], reference: &[f32], words: &str, lines: &[String], language: Option<&str>, device: Option<&str>, cancel: &AtomicBool) -> Result<(Vec<f32>, u32), String> {
    let parts = read_lines(exe, files, reference, words, lines, language, device, cancel).await?;
    let gap = vec![0f32; (TTS_RATE as f32 * 0.35) as usize];
    let mut all = Vec::new();
    for p in parts.into_iter().filter(|p| !p.is_empty()) {
        if !all.is_empty() {
            all.extend_from_slice(&gap);
        }
        all.extend(p);
    }
    if all.is_empty() {
        return Err("The voice engine made no sound.".into());
    }
    Ok((all, TTS_RATE))
}

/// Each of `lines` read in the voice of `reference` (24 kHz), checked as
/// `read_with` says: one reading per line (empty where the engine made
/// none), each kept to its line's length.
#[allow(clippy::too_many_arguments)]
pub(super) async fn read_lines(exe: &Path, files: &[PathBuf], reference: &[f32], words: &str, lines: &[String], language: Option<&str>, device: Option<&str>, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    // The recording goes to the engine as a file for the run only.
    let work = std::env::temp_dir().join(format!("sulcusai-voice-{}", super::super::store::new_id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = async {
        std::fs::write(work.join("ref.wav"), crate::audio::wav_encode(reference, TTS_RATE)).map_err(|e| e.to_string())?;
        std::fs::write(work.join("ref.txt"), words).map_err(|e| e.to_string())?;
        let cloned = How::Clone { work: &work, with_words: !words.trim().is_empty() };
        let mut parts = run_engine(exe, files, cloned, lines, language, device, cancel).await?;
        let ran_on = |line: &str, p: &[f32]| p.len() as f32 / TTS_RATE as f32 > longest_s(line) - 0.5;
        let again: Vec<usize> = (0..lines.len().min(parts.len())).filter(|i| ran_on(&lines[*i], &parts[*i])).collect();
        if !again.is_empty() {
            crate::applog::line("media", &format!("{} of {} lines ran on in the recorded voice; reading them again from the voice alone", again.len(), lines.len()));
            let retry: Vec<String> = again.iter().map(|i| lines[*i].clone()).collect();
            let redone = run_engine(exe, files, How::Clone { work: &work, with_words: false }, &retry, language, device, cancel).await?;
            for (i, p) in again.iter().zip(redone) {
                if !ran_on(&lines[*i], &p) || p.len() < parts[*i].len() {
                    parts[*i] = p;
                }
            }
        }
        if !heard_any(&parts) && device != Some("CPU") && exe.to_string_lossy().contains("vulkan") {
            crate::applog::line("media", "The graphics card's voice engine made only silence; reading it again on the processor");
            parts = run_engine(exe, files, cloned, lines, language, Some("CPU"), cancel).await?;
        }
        if !heard_any(&parts) {
            crate::applog::line("media", &format!("The voice engine made only silence for {} line(s)", lines.len()));
            return Err("The voice engine made only silence. Try again; if it keeps happening, record your voice's lines again in Studio › Your voice.".into());
        }
        // Samples that aren't numbers in a line that otherwise worked become silence.
        parts.iter_mut().flatten().filter(|s| !s.is_finite()).for_each(|s| *s = 0.0);
        parts.resize(lines.len(), Vec::new());
        for (p, line) in parts.iter_mut().zip(lines) {
            // Still running on: kept to its line's length.
            p.truncate((longest_s(line) * TTS_RATE as f32) as usize);
        }
        Ok(parts)
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Several passages read in a recorded (or made) voice in one go, so the
/// model loads once: one reading per passage at 24 kHz, its sentences a
/// short pause apart. For podcasts and readings with several voices.
pub async fn speak_each(state: &Arc<AppState>, voice_id: &str, passages: &[String], language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(Vec<Vec<f32>>, u32), String> {
    let c = state.work_cipher()?;
    hear_missing_words(state, &c, voice_id, &|_| progress("Hearing the words of your recordings (once)", 0.05)).await;
    let voice = get(&state.db.lock().unwrap(), &c, voice_id).ok_or("That voice is no longer here.")?;
    let (reference, words) = speech_reference(&state.paths, &c, &voice)?;
    speak_each_like(state, &reference, &words, passages, language, cancel, progress).await
}

/// Whether voices read in a recorded voice can speak `language` (Qwen3-TTS
/// knows ten languages).
pub fn speaks(language: &str) -> bool {
    tts_language(Some(language)) != "auto"
}

/// `passages` read as `speak_each` does, in the voice of `reference` (24
/// kHz, a few seconds of someone speaking) whose words are `words` (empty
/// when they aren't known): a voice heard in a video, for dubbing.
#[allow(clippy::too_many_arguments)]
pub async fn speak_each_like(state: &Arc<AppState>, reference: &[f32], words: &str, passages: &[String], language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(Vec<Vec<f32>>, u32), String> {
    // Every passage's sentences in one list, and which passage each is from.
    let mut lines = Vec::new();
    let mut owner = Vec::new();
    for (i, p) in passages.iter().enumerate() {
        for l in lines_of(p) {
            lines.push(l);
            owner.push(i);
        }
    }
    if lines.is_empty() {
        return Ok((vec![Vec::new(); passages.len()], TTS_RATE));
    }
    let (exe, files) = speak_parts(state, cancel, progress).await?;
    let free = card_free_now(state);
    let device = (exe.to_string_lossy().contains("vulkan") && free.is_some_and(|f| f < 3_500 << 20)).then_some("CPU");
    let parts = read_lines(&exe, &files, reference, words, &lines, language, device, cancel).await?;
    let gap = vec![0f32; (TTS_RATE as f32 * 0.3) as usize];
    let mut out = vec![Vec::new(); passages.len()];
    for (p, i) in parts.into_iter().zip(owner) {
        if p.is_empty() {
            continue;
        }
        if !out[i].is_empty() {
            out[i].extend_from_slice(&gap);
        }
        out[i].extend(p);
    }
    Ok((out, TTS_RATE))
}


/// What a made voice reads for its sample (~12 s, kept as its reference):
/// language code and words.
pub(super) const SAMPLES: &[(&str, &str)] = &[
    ("en", "Hello, and thank you for listening. Today I want to tell you about a small town by the sea, where the mornings are quiet, the bread is always warm, and everybody knows your name."),
    ("de", "Hallo und danke fürs Zuhören. Heute erzähle ich Ihnen von einer kleinen Stadt am Meer, wo die Morgen ruhig sind, das Brot immer warm ist und jeder Ihren Namen kennt."),
    ("es", "Hola, y gracias por escuchar. Hoy quiero hablarles de un pequeño pueblo junto al mar, donde las mañanas son tranquilas, el pan siempre está caliente y todos conocen tu nombre."),
    ("fr", "Bonjour, et merci de votre écoute. Aujourd'hui, je veux vous parler d'une petite ville au bord de la mer, où les matins sont calmes, le pain toujours chaud, et où tout le monde connaît votre nom."),
    ("it", "Ciao, e grazie per l'ascolto. Oggi voglio parlarvi di una piccola città sul mare, dove le mattine sono tranquille, il pane è sempre caldo e tutti conoscono il tuo nome."),
    ("pt", "Olá, e obrigado por ouvir. Hoje quero falar de uma pequena cidade à beira-mar, onde as manhãs são tranquilas, o pão está sempre quente e todos sabem o seu nome."),
    ("ru", "Здравствуйте и спасибо, что слушаете. Сегодня я расскажу вам о маленьком городке у моря, где утро всегда тихое, хлеб всегда тёплый, а все знают ваше имя."),
    ("ja", "こんにちは、聞いてくださってありがとうございます。今日は、海辺の小さな町のお話をします。朝は静かで、パンはいつも温かく、みんながあなたの名前を知っています。"),
    ("ko", "안녕하세요, 들어 주셔서 감사합니다. 오늘은 바닷가의 작은 마을 이야기를 들려 드릴게요. 아침은 조용하고, 빵은 늘 따뜻하며, 모두가 당신의 이름을 알고 있어요."),
    ("zh", "你好，谢谢你的收听。今天我想给你讲一个海边小镇的故事。那里的早晨很安静，面包总是热的，每个人都知道你的名字。"),
];

/// The sample's words in `language` (English when it isn't one of them).
pub(super) fn sample_words(language: Option<&str>) -> &'static str {
    let code = language.map(|c| c.split(['-', '_']).next().unwrap_or("").to_lowercase()).unwrap_or_default();
    SAMPLES.iter().find(|(c, _)| *c == code).unwrap_or(&SAMPLES[0]).1
}

/// Readings made from the last description, waiting to be kept.
pub(super) struct Made {
    pub(super) description: String,
    pub(super) language: Option<String>,
    pub(super) takes: Vec<Vec<f32>>,
}

pub(super) static MADE: Mutex<Option<Made>> = Mutex::new(None);
pub(super) static DESIGN_CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
pub struct MadeSample {
    /// WAV, base64.
    pub(super) wav: String,
    pub(super) seconds: f32,
}

/// Makes three voices from a description, each reading the same sample, to
/// choose from. "own_voice:designing" events say what it's doing.
#[tauri::command]
pub async fn own_voice_design(app: AppHandle, state: AppStateRef<'_>, description: String, language: Option<String>) -> Result<Vec<MadeSample>, String> {
    use base64::Engine as _;
    crate::features::require(&state, crate::features::Feature::Music)?;
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(400).collect::<String>();
    if description.chars().count() < 4 {
        return Err("Describe the voice: who it sounds like (age, man or woman), its pitch, pace and mood.".into());
    }
    let state = state.inner().clone();
    DESIGN_CANCEL.store(false, Ordering::Relaxed);
    let say = |stage: &str, fraction: f64| {
        let _ = app.emit("own_voice:designing", json!({ "stage": stage, "fraction": fraction }));
    };
    let takes = design(&state, &description, language.as_deref(), &DESIGN_CANCEL, &say).await?;
    let out = takes
        .iter()
        .map(|t| MadeSample { wav: base64::engine::general_purpose::STANDARD.encode(crate::audio::wav_encode(t, TTS_RATE)), seconds: t.len() as f32 / TTS_RATE as f32 })
        .collect();
    *MADE.lock().unwrap() = Some(Made { description, language, takes });
    state.log("media", "Made voices from a description");
    Ok(out)
}

/// Three readings of the sample in voices made from `description` (24 kHz).
pub async fn design(state: &Arc<AppState>, description: &str, language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<Vec<f32>>, String> {
    let (exe, files) = design_parts(state, cancel, progress).await?;
    progress("Making the voices", 0.0);
    let words = sample_words(language).to_string();
    let lines = vec![words.clone(), words.clone(), words];
    let mut takes = run_engine(&exe, &files, How::Design(description), &lines, language, None, cancel).await?;
    if !heard_any(&takes) && exe.to_string_lossy().contains("vulkan") {
        crate::applog::line("media", "The graphics card's voice maker made only silence; making it again on the processor");
        takes = run_engine(&exe, &files, How::Design(description), &lines, language, Some("CPU"), cancel).await?;
    }
    takes.retain(|t| !t.is_empty() && t.iter().all(|s| s.is_finite()) && crate::audio::rms(t) > 0.003);
    if takes.is_empty() {
        return Err("The voice maker made only silence. Try again, or describe the voice another way.".into());
    }
    for t in &mut takes {
        t.truncate((longest_s(&lines[0]) * TTS_RATE as f32) as usize);
    }
    Ok(takes)
}

/// Keeps a made reading as a voice (for tests: the command keeps one of the
/// last made).
#[cfg(test)]
pub fn keep_for_test(state: &AppState, name: &str, description: &str, sample: &[f32]) -> String {
    *MADE.lock().unwrap() = Some(Made { description: description.into(), language: Some("en".into()), takes: vec![sample.to_vec()] });
    let at48 = stems::resample(sample, TTS_RATE, REC_RATE);
    let id = add_for_test(state, name, &[(&format!("{ADDED}1"), at48, sample_words(Some("en")))]);
    let c = state.work_cipher().unwrap();
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    if let Some(v) = all.iter_mut().find(|v| v.id == id) {
        v.designed = Some(description.into());
        for t in &mut v.takes {
            t.kind = Some(LineKind::Speak);
            t.score = 1.0;
        }
    }
    store(&conn, &c, &all).unwrap();
    id
}

#[tauri::command]
pub fn own_voice_design_cancel() {
    DESIGN_CANCEL.store(true, Ordering::Relaxed);
}

/// Keeps one of the voices just made (`pick`, from 0) under `name`: its
/// reading becomes the voice's reference, so it sounds the same every time.
#[tauri::command]
pub fn own_voice_design_keep(state: AppStateRef<'_>, name: String, pick: usize) -> Result<OwnVoicesView, String> {
    let name = name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err("Give the voice a name.".into());
    }
    let made = MADE.lock().unwrap().take().ok_or("Make the voices again first.")?;
    if pick >= made.takes.len() {
        *MADE.lock().unwrap() = Some(made);
        return Err("That voice isn't one of the ones made.".into());
    }
    let at48 = stems::resample(&made.takes[pick], TTS_RATE, REC_RATE);
    let c = state.work_cipher()?;
    let id = super::super::store::new_id();
    let line = format!("{ADDED}1");
    let path = take_path(&state.paths, &id, &line)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&path, c.seal_bytes(&crate::audio::wav_encode(&at48, REC_RATE))).map_err(|e| e.to_string())?;
    let now = db::now_ms();
    let take = Take {
        line,
        seconds: at48.len() as f32 / REC_RATE as f32,
        words: sample_words(made.language.as_deref()).to_string(),
        at: now,
        kind: Some(LineKind::Speak),
        score: 1.0,
        ..Default::default()
    };
    {
        let conn = state.db.lock().unwrap();
        let mut all = list(&conn, &c);
        all.push(OwnVoice { id, name, takes: vec![take], created_at: now, agreed_at: now, designed: Some(made.description) });
        store(&conn, &c, &all)?;
    }
    state.log("media", "Kept a voice made from a description");
    view(&state)
}

/// Hears a sentence in a voice, to check it: WAV, base64. "own_voice:trying"
/// events say what it's doing: downloading the engine or model the first
/// time (with how far), then reading.
#[tauri::command]
pub async fn own_voice_try(app: AppHandle, state: AppStateRef<'_>, voice: String, text: String, language: Option<String>) -> Result<String, String> {
    use base64::Engine as _;
    crate::features::require(&state, crate::features::Feature::Music)?;
    let state = state.inner().clone();
    let never = AtomicBool::new(false);
    let say = |stage: &str, fraction: f64| {
        // While it reads there's no telling how far it is (-1).
        let downloading = stage.contains("(once");
        let _ = app.emit("own_voice:trying", json!({ "stage": stage, "fraction": if downloading { fraction } else { -1.0 }, "downloading": downloading }));
    };
    let (samples, rate) = speak(&state, &voice, &text, language.as_deref(), &never, &say).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(crate::audio::wav_encode(&samples, rate)))
}

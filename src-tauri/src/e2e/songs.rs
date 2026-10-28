// SPDX-License-Identifier: AGPL-3.0-only
//! Songs: lyrics, planning, voices and whole songs.

use super::*;

/// A song attached to a chat is worked on: "make this longer" calls
/// create_music on that song, and the result is that much longer. A small
/// model (SULCUSAI_TEST_GGUF, Qwen3 4B) so the music engine fits beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_chat_works_on_an_attached_song() {
    use crate::media::{self, store, Request};
    let (_eng, ep, name) = test_model().await;
    let state = media_state();
    let cipher = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    // A song of theirs, attached to the chat.
    let song = media::run(&state, store::new_id(), Request { op: "music".into(), prompt: "calm acoustic guitar instrumental".into(), seconds: Some(20.0), instrumental: Some(true), ..Default::default() }).await.unwrap().remove(0);
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let mut item = song.clone();
    item.chat_id = Some(chat.id.clone());
    item.hidden = true;
    store::update(&state.db.lock().unwrap(), &cipher, &item).unwrap();
    let turn_id = uuid::Uuid::new_v4().to_string();
    crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message {
        id: turn_id.clone(),
        chat_id: chat.id.clone(),
        role: "user".into(),
        content: "Make this song 20 seconds longer.".into(),
        created_at: crate::db::now_ms(),
        meta: Some(serde_json::json!({ "images": [song.id] })),
        ..Default::default()
    })
    .unwrap();
    let turn = crate::agent::Turn {
        emit: std::sync::Arc::new(|_: &str, _| {}),
        state: state.clone(),
        chat_id: chat.id.clone(),
        turn_id,
        cipher: cipher.clone(),
        ep: ep.clone(),
        mode: crate::tools::Mode::Auto,
        use_tools: true,
        vision: false,
        unattended: false,
        pre_approved: None,
        base: chat::system_prompt(&Profile::default(), "today").0,
        about: String::new(),
        cancel: std::sync::Arc::new(AtomicBool::new(false)),
    };
    let started = std::time::Instant::now();
    turn.run().await.unwrap();
    let (_, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: make the attached song longer ({:.0}s)", started.elapsed().as_secs_f64()));
    let made: Vec<media::MediaItem> = store::in_chat(&state.db.lock().unwrap(), &cipher, &chat.id).into_iter().filter(|i| i.kind == "audio" && i.id != song.id).collect();
    println!("made: {:?}", made.iter().map(|i| (i.parent.clone(), i.seconds, i.prompt.clone())).collect::<Vec<_>>());
    assert!(tools.iter().any(|t| t == "create_music"), "worked on it with create_music: {tools:?}");
    let longer = made.iter().find(|i| i.parent.as_deref() == Some(song.id.as_str())).expect("a song made from the attached one");
    assert!((longer.seconds - (song.seconds + 20.0)).abs() < 3.0, "{} s from {} s", longer.seconds, song.seconds);
}

/// Asked in a chat to refine pasted lyrics, the model answers with the
/// lyrics in words: no song, no picture, even with music and picture models
/// installed and lyrics that say "paint the sky" and "picture of us" (that
/// made a picture: the check for a picture request read the pasted lines).
/// SULCUSAI_TEST_GGUF is the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_lyrics_are_refined_as_text() {
    use crate::media;
    let (_eng, ep, name) = test_model().await;
    let state = media_state();
    let cipher = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    media::install_for_test(&state, "z-image-turbo").await.unwrap();
    let asks = [
        "Can you refine these lyrics? Make them flow better but keep the meaning.\n\n[Verse]\nI want to paint the sky with you tonight\nEvery picture of us shining bright\nWe drive along the empty road\nCarrying the dreams we never told\n\n[Chorus]\nHold on, hold on, don't let go\nWe're the only light I know",
        "[Verse]\nThe kitchen floor is cold beneath our feet\nGrandma hums a song so soft and sweet\nThe radio plays an old waltz tune\nWe spin around beneath the moon\n\nThese are my lyrics. Make them rhyme better and fix the rhythm of the second line.",
        "Rewrite this chorus so it's catchier:\nDraw me a picture of the place we met\nThe sun was setting, I can't forget",
    ];
    let mut bad = Vec::new();
    for (i, ask) in asks.iter().enumerate() {
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        let turn_id = uuid::Uuid::new_v4().to_string();
        crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message { id: turn_id.clone(), chat_id: chat.id.clone(), role: "user".into(), content: ask.to_string(), created_at: crate::db::now_ms(), ..Default::default() }).unwrap();
        let turn = crate::agent::Turn {
            emit: std::sync::Arc::new(|_: &str, _| {}),
            state: state.clone(),
            chat_id: chat.id.clone(),
            turn_id,
            cipher: cipher.clone(),
            ep: ep.clone(),
            mode: crate::tools::Mode::Auto,
            use_tools: true,
            vision: false,
            unattended: false,
            pre_approved: None,
            base: chat::system_prompt(&Profile::default(), "today").0,
            about: String::new(),
            cancel: std::sync::Arc::new(AtomicBool::new(false)),
        };
        let started = std::time::Instant::now();
        turn.run().await.unwrap();
        let (_, tools) = transcript(&state, &cipher, &chat.id, &format!("{name}: lyrics {i} ({:.0}s)", started.elapsed().as_secs_f64()));
        let reply = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id).into_iter().rev().find(|m| m.role == "assistant").map(|m| m.content).unwrap_or_default();
        if tools.iter().any(|t| t.starts_with("create_") || t == "edit_image") || reply.lines().filter(|l| !l.trim().is_empty()).count() < 2 {
            bad.push(format!("{i}: {tools:?}"));
        }
    }
    assert!(bad.is_empty(), "made media instead of refining the words: {bad:?}");
}

/// "Write me a song" is asked about first (just the lyrics, or the song to
/// hear?) rather than made outright; told "just the lyrics", every line comes,
/// even past a small length limit (carried on where it stopped).
///     SULCUSAI_TEST_GGUF is the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_song_written_is_asked_about_and_comes_whole() {
    use crate::media;
    let (_eng, ep, name) = test_model().await;
    let state = media_state();
    let cipher = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let made = |state: &std::sync::Arc<crate::AppState>| crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id).iter().flat_map(|m| m.tool_calls.iter().flatten().map(|c| c.name.clone())).filter(|n| n.starts_with("create_")).collect::<Vec<_>>();
    let first = say(&state, &ep, &cipher, &chat.id, "Write me a song about my dog Max").await;
    println!("{name} asked:\n{first}\n");
    assert!(made(&state).is_empty(), "made something instead of asking: {:?}", made(&state));
    assert!(first.contains('?') && first.to_lowercase().contains("lyric"), "didn't ask lyrics or the song: {first}");
    // A small limit, so the lyrics run past it and are carried on.
    let mut short = ep.clone();
    short.extra["max_tokens"] = serde_json::json!(160);
    let words = say(&state, &short, &cipher, &chat.id, "Just the lyrics please: upbeat pop, three verses, a chorus and a bridge.").await;
    println!("{name} wrote:\n{words}");
    assert!(made(&state).is_empty(), "made something instead of writing: {:?}", made(&state));
    let lower = words.to_lowercase();
    assert!(lower.contains("chorus") && lower.contains("bridge"), "the lyrics stopped partway");
    assert!(words.chars().count() > 160 * 3, "not carried on past the limit ({} characters)", words.chars().count());
}

/// "See how it's built" on an NVIDIA card: a song's parts taken apart and
/// their notes read through CUDA (its libraries downloaded the first time),
/// timed. SULCUSAI_SONG is the MP3 and SULCUSAI_SONG_SECONDS its length;
/// SULCUSAI_ONNX_CPU=1 keeps it on the processor, to compare (run that
/// alone: a process loads one ONNX Runtime).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_song_parts_on_the_card() {
    use crate::media::{self, store, MediaItem};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    let c = state.work_cipher().unwrap();
    let song = std::fs::read(std::env::var("SULCUSAI_SONG").expect("SULCUSAI_SONG")).unwrap();
    let seconds = std::env::var("SULCUSAI_SONG_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let item = MediaItem { kind: "audio".into(), mime: "audio/mpeg".into(), op: "import".into(), prompt: "a song".into(), seconds, ..Default::default() };
    let item = store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &song, None).unwrap();
    let on_processor = std::env::var_os("SULCUSAI_ONNX_CPU").is_some();
    for round in 1..=2 {
        let t = std::time::Instant::now();
        let mapped = media::run(&state, store::new_id(), media::Request { op: "map".into(), source: Some(item.id.clone()), ..Default::default() }).await.unwrap().remove(0);
        let map = mapped.map.unwrap();
        println!(
            "round {round}: mapped in {:.1}s ({}); {}",
            t.elapsed().as_secs_f64(),
            if crate::natural::loaded_for_cards() { "the card's runtime" } else { "the processor's runtime" },
            map.parts.iter().map(|p| format!("{} {}", p.name, p.notes.len())).collect::<Vec<_>>().join(", ")
        );
        assert!(map.parts.iter().map(|p| p.notes.len()).sum::<usize>() > 100);
    }
    let log = std::fs::read_to_string(state.paths.logs.join("app.log")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().filter(|l| l.contains("song's parts") || l.contains("song apart") || l.contains("graphics card")).collect();
    for line in &lines {
        println!("{line}");
    }
    let want = if on_processor { "on the processor" } else { "on the graphics card" };
    let ran: Vec<&&str> = lines.iter().filter(|l| l.contains("Took a song's parts apart")).collect();
    assert!(ran.len() == 2 && ran.iter().all(|l| l.ends_with(want)), "{want}");
}

/// An MP3 as mono samples and their rate, decoded by Windows (Media Foundation).
#[cfg(windows)]
fn decode_mp3(bytes: &[u8]) -> (Vec<f32>, u32) {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    let path = std::env::temp_dir().join(format!("sulcusai-song-{}.mp3", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes).unwrap();
    let mut mono = Vec::new();
    let rate;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).unwrap();
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).unwrap();
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        let want = MFCreateMediaType().unwrap();
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
        want.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).unwrap();
        reader.SetCurrentMediaType(stream, None, &want).unwrap();
        let got = reader.GetCurrentMediaType(stream).unwrap();
        rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap();
        let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap().max(1) as usize;
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader.ReadSample(stream, 0, None, Some(&mut flags as *mut u32), None, Some(&mut sample as *mut Option<IMFSample>)).unwrap();
            if let Some(s) = sample {
                let buf = s.ConvertToContiguousBuffer().unwrap();
                let (mut p, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
                buf.Lock(&mut p, None, Some(&mut len as *mut u32)).unwrap();
                let floats = std::slice::from_raw_parts(p as *const f32, len as usize / 4);
                mono.extend(floats.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
                buf.Unlock().unwrap();
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        drop(reader);
        let _ = MFShutdown();
    }
    std::fs::remove_file(&path).ok();
    (mono, rate)
}

/// A rough print of a singing voice (from a vocals-only track): its middle
/// pitch over the voiced moments (autocorrelation, 80 to 800 Hz) and how
/// often it crosses zero there (brightness, breathiness). Two songs by one
/// singer land close together; different singers, further apart.
#[cfg(windows)]
fn voice_print(samples: &[f32], rate: u32) -> (f32, f32) {
    let frame = (rate as usize) / 20;
    let loudest = samples.chunks(frame).map(crate::audio::rms).fold(0.0f32, f32::max).max(1e-6);
    let (lo, hi) = (rate as usize / 800, rate as usize / 80);
    let mut pitches = Vec::new();
    let mut crossings = Vec::new();
    for f in samples.chunks(frame).filter(|f| f.len() == frame && crate::audio::rms(f) > loudest * 0.25) {
        let energy: f32 = f.iter().map(|x| x * x).sum::<f32>().max(1e-9);
        let (lag, peak) = (lo..hi.min(frame - 1)).map(|l| (l, f.iter().zip(&f[l..]).map(|(a, b)| a * b).sum::<f32>() / energy)).fold((0, 0.0f32), |best, x| if x.1 > best.1 { x } else { best });
        if peak > 0.5 && lag > 0 {
            pitches.push(rate as f32 / lag as f32);
            crossings.push(f.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f32 / frame as f32);
        }
    }
    let middle = |mut v: Vec<f32>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    (middle(pitches), middle(crossings))
}

/// Lyrics from a real chat model fill the song: a minute gets a verse and
/// a chorus (it was three lines), three minutes the whole shape, chosen
/// sections come in order. An artist's style is read without their name,
/// and a song in their way, with a feeling and a screamed chorus, is
/// written and its tags marked.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_lyrics_fill_the_song() {
    use crate::media::{artist, lyrics, VocalEffect};
    let state = media_state();
    lyrics_model(&state);
    let mut failed = Vec::new();
    let structure: Vec<String> = ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus", "Outro"].map(String::from).to_vec();
    let cases: [(f32, Vec<String>, &str); 4] = [
        (30.0, vec![], "cheerful ukulele pop about a sunny weekend"),
        (60.0, vec![], "upbeat acoustic folk song about a road trip through the mountains, warm male vocals"),
        (180.0, vec![], "a heartfelt pop ballad about moving away from home"),
        (150.0, structure, "anthemic rock song about old friends"),
    ];
    for (secs, sections, prompt) in cases {
        let plan = lyrics::plan(secs, &sections, lyrics::Flow::Sung);
        let planned: u32 = plan.iter().map(|(_, n)| n).sum();
        let t = std::time::Instant::now();
        match lyrics::write(&state, prompt, Some(secs), Some("en"), &sections, "", lyrics::Flow::Sung).await {
            Ok(l) => {
                let n = lyrics::sung_lines(&l);
                let tags: Vec<&str> = l.lines().filter(|x| x.trim().starts_with('[')).collect();
                println!("{secs:.0}s: {n} of {planned} lines in {:.0}s, tags {tags:?}\n{l}\n", t.elapsed().as_secs_f64());
                if (n as f32) < planned as f32 * 0.75 {
                    failed.push(format!("{secs:.0}s: {n} lines, {planned} planned"));
                }
            }
            Err(e) => failed.push(format!("{secs:.0}s: {e:?}")),
        }
    }
    for name in ["Taylor Swift", "Lewis Capaldi"] {
        match artist::look_up(&state, name).await {
            Ok(Some(s)) => {
                println!("{name}: {s:#?}");
                let last = name.split(' ').last().unwrap();
                if s.sound().contains(last) || s.genre.is_empty() {
                    failed.push(format!("{name}: {s:?}"));
                }
            }
            other => {
                println!("{name}: not read: {other:?}; it said {:?}", artist::ask(&state, name).await);
                failed.push(format!("{name}: {other:?}"));
            }
        }
    }
    let req = crate::media::Request { op: "music".into(), prompt: "Sing as Lewis Capaldi a song about losing someone at the end of summer".into(), ..Default::default() };
    let Some(styled) = artist::resolve(&state, &req, &|_, _| {}).await else {
        state.engine.lock().await.stop().await;
        panic!("the artist wasn't found: {failed:#?}");
    };
    let mut spec = styled.music.clone().unwrap();
    println!("prompt now {:?}; caption: {}", styled.prompt, spec.caption(&styled.prompt, false));
    if styled.prompt.contains("Capaldi") || spec.caption(&styled.prompt, false).contains("Capaldi") {
        failed.push(format!("the name reached the music model: {:?}", styled.prompt));
    }
    spec.emotion = Some("Heartbroken".into());
    spec.rhyme = Some("aabb".into());
    spec.vocal_effects = vec![VocalEffect { how: "screamed".into(), on: "chorus".into() }, VocalEffect { how: "vibrato".into(), on: "all".into() }];
    match lyrics::write(&state, &spec.caption(&styled.prompt, false), Some(90.0), Some("en"), &spec.sections(), &spec.lyrics_guide(), spec.flow(&styled.prompt)).await {
        Ok(l) => {
            let marked = spec.mark_sections(&l);
            println!("in their way, heartbroken, screamed chorus:\n{marked}\n");
            if !marked.contains("[Chorus - screamed]") {
                failed.push("the chorus tag isn't marked".into());
            }
        }
        Err(e) => failed.push(format!("styled: {e:?}")),
    }
    state.engine.lock().await.stop().await;
    assert!(failed.is_empty(), "{failed:#?}");
}

/// ACE-Step rendered from noise against the same song planned first by its
/// planning model (ace-lm, 1.7B and 4B), as ACE-Step 1.5 is meant to run:
/// the same lyrics, seeds and settings, with the time, the length it came
/// out and the lyric words Whisper hears. The songs are kept in
/// C:\dev\tools\music\ab for listening. SULCUSAI_PLAN_DIT picks the
/// rendering model file (default XL turbo), SULCUSAI_PLANNERS the planners.
///     cargo test e2e_music_planned_or_not -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_planned_or_not() {
    use crate::media::music;
    let state = media_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    let dit = std::env::var("SULCUSAI_PLAN_DIT").unwrap_or_else(|_| "acestep-v15-xl-turbo-Q8_0.gguf".into());
    let planners = std::env::var("SULCUSAI_PLANNERS").unwrap_or_else(|_| ",acestep-5Hz-lm-1.7B-Q8_0.gguf,acestep-5Hz-lm-4B-Q8_0.gguf".into());
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me\n\n[Verse 2]\nOld guitar by the doorway\nStrings still tuned to the past\nEvery chord is a promise\nThat the good days will last\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let caption = "modern country, warm male lead vocal, acoustic guitar, pedal steel, punchy live drums, warm bass, catchy chorus, polished Nashville production";
    let seconds = 100.0;
    let wanted = sung_words(lyrics);
    let mut rows = Vec::new();
    for planner in planners.split(',') {
        if !planner.is_empty() && !models.join(planner).is_file() {
            rows.push(format!("{planner}: not downloaded"));
            continue;
        }
        let name = if planner.is_empty() { "noise".to_string() } else { planner.trim_start_matches("acestep-5Hz-lm-").trim_end_matches("-Q8_0.gguf").to_string() };
        for seed in [11i64, 22] {
            let (bpm, key, time) = music::settle(caption, None, None, None, seed);
            let work = std::env::temp_dir().join(format!("sulcusai-plan-{}", crate::media::store::new_id()));
            std::fs::create_dir_all(&work).unwrap();
            let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
            let r = music::SongRequest {
                caption,
                lyrics,
                seconds,
                seed,
                language: Some("en"),
                dit_file: &dit,
                bpm: Some(bpm),
                key: Some(&key),
                time_signature: Some(&time),
                versions: 1,
                planner,
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let made = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await;
            let took = t.elapsed().as_secs_f64();
            let _ = std::fs::remove_dir_all(&work);
            let song = match made {
                Ok(mut s) => s.remove(0),
                Err(e) => {
                    rows.push(format!("{name} seed {seed}: failed ({e})"));
                    println!("{}", rows.last().unwrap());
                    continue;
                }
            };
            let file = out.join(format!("{name}-{seed}.mp3"));
            std::fs::write(&file, &song.mp3).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let words = sung_words(&heard);
            let matched = wanted.iter().filter(|w| words.contains(*w)).count();
            rows.push(format!("{name} seed {seed}: {took:.0} s, {:.0} s long ({seconds:.0} asked), {matched} of {} lyric words heard", song.seconds, wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    println!("\n{}\nsongs in {}", rows.join("\n"), out.display());
}

/// One music model at several guidance strengths: the same lyrics and seeds,
/// with the time and the lyric words Whisper hears. Songs kept in
/// C:\dev\tools\music\ab as guide-<g>-<seed>.mp3. SULCUSAI_GUIDE_DIT picks
/// the model file (default ACE-Step 1.5 Base), SULCUSAI_GUIDES the strengths.
///     cargo test e2e_music_guidance -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_guidance() {
    use crate::media::music;
    let state = media_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    let dit = std::env::var("SULCUSAI_GUIDE_DIT").unwrap_or_else(|_| "acestep-v15-base-Q8_0.gguf".into());
    let guides = std::env::var("SULCUSAI_GUIDES").unwrap_or_else(|_| "1,3,7".into());
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me\n\n[Verse 2]\nOld guitar by the doorway\nStrings still tuned to the past\nEvery chord is a promise\nThat the good days will last\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let caption = "modern country, warm male lead vocal, acoustic guitar, pedal steel, punchy live drums, warm bass, catchy chorus, polished Nashville production";
    let wanted = sung_words(lyrics);
    let mut rows = vec![format!("{dit}")];
    for g in guides.split(',').filter_map(|g| g.trim().parse::<f32>().ok()) {
        for seed in [11i64, 22] {
            let (bpm, key, time) = music::settle(caption, None, None, None, seed);
            let work = std::env::temp_dir().join(format!("sulcusai-guide-{}", crate::media::store::new_id()));
            std::fs::create_dir_all(&work).unwrap();
            let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
            let r = music::SongRequest { caption, lyrics, seconds: 100.0, seed, language: Some("en"), dit_file: &dit, bpm: Some(bpm), key: Some(&key), time_signature: Some(&time), versions: 1, guidance: Some(g), ..Default::default() };
            let t = std::time::Instant::now();
            let song = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await.unwrap().remove(0);
            let took = t.elapsed().as_secs_f64();
            let _ = std::fs::remove_dir_all(&work);
            let file = out.join(format!("guide-{g}-{seed}.mp3"));
            std::fs::write(&file, &song.mp3).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
            rows.push(format!("guidance {g} seed {seed}: {took:.0} s, {matched} of {} lyric words heard", wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    println!("\n{}", rows.join("\n"));
}

/// A song from lyrics written Suno's way (directions in the brackets), as
/// the Studio makes it: on YuE2 (installed from the catalog: its engine from
/// the engines release) and on ACE-Step 1.5 XL, each kept in
/// C:\dev\tools\music\ab with the words Whisper hears and its length.
///     cargo test e2e_suno_style_lyrics -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_suno_style_lyrics() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let lyrics = "[Intro: dark acoustic ambience, faint fire crackling, distant steel guitar]\n\n[Verse 1: low warm male vocal, conversational phrasing, restrained acoustic groove]\nI learned the shape of every fracture\nEvery place we came undone\nKept rebuilding what was breaking\nLike the work could make us one\n\n[Chorus: strong melodic lift, memorable country hook, full drums and vocal harmonies]\nSmoke and fire\nThe flames climbed higher\nBuilt a home on all the ashes\nWhile you kept striking the matches\n\n[Pause]\n\n[Outro: drums disappear, acoustic guitar and pedal steel fade]\nNothing left to save now";
    let wanted = sung_words(lyrics);
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    for model in ["yue2-3b", "ace-step-1.5-xl"] {
        let t = std::time::Instant::now();
        media::install_for_test(&state, model).await.unwrap_or_else(|e| panic!("installing {model}: {e}"));
        let req = Request {
            op: "music".into(),
            model_id: Some(model.into()),
            prompt: "dark modern country, Southern gothic".into(),
            lyrics: Some(lyrics.into()),
            seconds: Some(70.0),
            seed: Some(11),
            language: Some("en".into()),
            music: Some(media::MusicSpec { voice: Some("male".into()), ..Default::default() }),
            ..Default::default()
        };
        let item = media::run(&state, store::new_id(), req).await.unwrap_or_else(|e| panic!("{model}: {e}")).remove(0);
        let file = out.join(format!("smoke-{model}.mp3"));
        std::fs::write(&file, store::read(&state.paths, &c, &item.id).unwrap()).unwrap();
        let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
        let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
        let sang_tags = ["melodic", "crackling", "conversational", "disappear"].iter().filter(|w| heard.to_lowercase().contains(*w)).count();
        println!("{model}: {:.0} s long in {:.0} s, {matched} of {} lyric words heard, {sang_tags} direction words sung", item.seconds, t.elapsed().as_secs_f64(), wanted.len());
        assert!((item.seconds - 70.0).abs() < 3.0);
    }
}

/// The singer's middle pitch in each song in a folder (SULCUSAI_PITCH_DIR,
/// default C:\dev\tools\music\ab): the voice taken out (HTDemucs) and its
/// pitch found (RMVPE, from the singing model's pieces). Under ~200 Hz is a
/// man's range, over ~230 a woman's.
///     cargo test e2e_singer_pitch -- --ignored --nocapture
#[cfg(windows)]
#[test]
#[ignore]
fn e2e_singer_pitch() {
    use crate::media::{stems, svc};
    let state = media_state();
    let dir = std::path::PathBuf::from(std::env::var("SULCUSAI_PITCH_DIR").unwrap_or_else(|_| r"C:\dev\tools\music\ab".into()));
    let real = Paths::new(app_data_dir()).unwrap();
    let dll = [real.engines.join("onnxruntime-1.30.0-cuda13").join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
    let sep = state.paths.models.join("media").join("helpers").join("htdemucs_fp16weights.onnx");
    let mut conv = svc::Converter::new(&dll, &state.paths.models.join("media").join("own-voice-sing"), 8, card).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "mp3")).collect();
    files.sort();
    for f in files {
        let (mono, rate) = decode_mp3(&std::fs::read(&f).unwrap());
        let mix = stems::Stereo { rate, ch: [mono.clone(), mono] }.resampled(stems::RATE);
        let (parts, _) = stems::separate(&dll, &sep, &mix, 8, &[3], card, &AtomicBool::new(false)).unwrap();
        let pitch = conv.middle_pitch_of(&parts[0].mono()).unwrap();
        println!("{}: singer {pitch:.0} Hz", f.file_name().unwrap().to_string_lossy());
    }
    stems::unload();
}

/// A song sung in the installed app's recorded voice from the start: the
/// same male song made with no reference and with the person's own singing
/// as the music model's reference (`--ref-audio`, which sets timbre and
/// delivery), then each sung again in the voice. Reads the app's data read
/// only; prints the words heard, the singer's middle pitch and how close
/// the voice print is to the person's, before and after converting. Songs
/// in C:\dev\tools\music\ab.
///     cargo test e2e_own_voice_from_the_start -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_own_voice_from_the_start() {
    use crate::media::{music, own_voice, stems, svc};
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&real.data.join("keys.json"), crate::crypto::platform_protector(&real.data)).unwrap();
    let c = vault.work_cipher().expect("the app's data key is locked with a password");
    let voice = own_voice::list(&conn, &c).into_iter().next().expect("a recorded voice");
    let (reference, sung) = own_voice::singing_reference(&real, &c, &voice).unwrap();
    println!("reference: {:.1} s of {}", reference.len() as f32 / svc::RATE as f32, if sung { "singing" } else { "reading" });
    // The singer as the app picks it from the recording, said in the caption the app's way.
    let sex = own_voice::sex_of(&real, &c, &voice);
    println!("their voice reads as {sex:?}");
    let spec = crate::media::MusicSpec { voice: sex.map(str::to_string), genre: Some("country".into()), instruments: vec!["acoustic guitar".into(), "pedal steel".into()], ..Default::default() };
    let caption = spec.caption("A modern country song about a fresh start, punchy live drums, warm bass, catchy chorus", false);
    let caption = caption.as_str();
    println!("caption: {caption}");
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab");
    std::fs::create_dir_all(&out).unwrap();
    // The reference for the music model, written for the run only.
    let ref_file = std::env::temp_dir().join(format!("sulcusai-ref-{}.wav", crate::media::store::new_id()));
    std::fs::write(&ref_file, crate::audio::wav_encode(&reference, svc::RATE)).unwrap();
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let wanted = sung_words(lyrics);
    let rt = real.engines.join("onnxruntime-1.30.0-cuda13");
    let dll = [rt.join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
    let sep = state.paths.models.join("media").join("helpers").join("htdemucs_fp16weights.onnx");
    let svc_dir = state.paths.models.join("media").join("own-voice-sing");
    let mut conv = svc::Converter::new(&dll, &svc_dir, 8, card).unwrap();
    let person = conv.voice(&reference).unwrap();
    let print = |conv: &mut svc::Converter, x: &[f32]| {
        let w16 = stems::resample(x, svc::RATE, 16_000);
        let (n, fb) = crate::diarize::fbank_with(&w16, true);
        let e = conv.run("campplus.onnx", vec![("fbank", vec![1, n as i64, 80], fb)]).unwrap();
        let norm = e.iter().map(|v| v * v).sum::<f32>().sqrt();
        e.into_iter().map(|v| v / norm).collect::<Vec<f32>>()
    };
    let target = print(&mut conv, &reference);
    let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let mut rows = vec![format!("their voice: middle pitch {:.0} Hz", person.pitch)];
    for (name, with_ref) in [("no-reference", false), ("their-reference", true)] {
        for seed in [11i64, 22] {
            let (bpm, key, time) = music::settle(caption, None, None, None, seed);
            let work = std::env::temp_dir().join(format!("sulcusai-ref-{}", crate::media::store::new_id()));
            std::fs::create_dir_all(&work).unwrap();
            let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
            let r = music::SongRequest {
                caption,
                lyrics,
                seconds: 60.0,
                seed,
                language: Some("en"),
                dit_file: "acestep-v15-xl-turbo-Q8_0.gguf",
                bpm: Some(bpm),
                key: Some(&key),
                time_signature: Some(&time),
                versions: 1,
                reference: with_ref.then_some(ref_file.as_path()),
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let song = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await.unwrap().remove(0);
            let took = t.elapsed().as_secs_f64();
            let _ = std::fs::remove_dir_all(&work);
            let file = out.join(format!("voice-{name}-{seed}.mp3"));
            std::fs::write(&file, &song.mp3).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
            // The singer, taken out of the band.
            let (mono, rate) = decode_mp3(&song.mp3);
            let mix = stems::Stereo { rate, ch: [mono.clone(), mono] }.resampled(stems::RATE);
            let (parts, _) = stems::separate(&dll, &sep, &mix, 8, &[3], card, &AtomicBool::new(false)).unwrap();
            stems::unload();
            let vocals = parts[0].mono();
            // How loud the band is beside the voice (a reference of bare
            // singing could thin the arrangement out).
            let whole = mix.mono();
            let band: Vec<f32> = whole.iter().zip(&vocals).map(|(m, v)| m - v).collect();
            let band_db = 20.0 * (crate::audio::rms(&band) / crate::audio::rms(&vocals).max(1e-6)).log10();
            let pitch = conv.middle_pitch_of(&vocals).unwrap();
            let before = cos(&print(&mut conv, &vocals), &target);
            let semis = svc::octave_shift(pitch, if sung { person.pitch } else { person.pitch * 1.26 });
            let converted = conv.convert(&person, &vocals, svc::Settings { steps: if card.is_some() { 30 } else { 10 }, semitones: semis, ..Default::default() }, &AtomicBool::new(false), &|_| {}).unwrap();
            let after = cos(&print(&mut conv, &converted), &target);
            rows.push(format!("{name} seed {seed}: {took:.0} s, {matched} of {} words heard, singer {pitch:.0} Hz, band {band_db:+.1} dB against the voice, likeness {before:.2} as made, {after:.2} sung again ({semis} semitones)", wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    let _ = std::fs::remove_file(&ref_file);
    println!("\n{}", rows.join("\n"));
}

/// Which music model, and which words, give the most natural voice above
/// the band: the same lyrics and seeds on each, with the words Whisper hears,
/// the voice's loudness against the whole song (from the vocals taken out by
/// ACE-Step 1.5 Base) and the time. SULCUSAI_VOICE_MODELS picks the models.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_voice_quality() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::env::temp_dir().join("sulcusai-voice-quality");
    std::fs::create_dir_all(&out).unwrap();
    let models = std::env::var("SULCUSAI_VOICE_MODELS").unwrap_or_else(|_| "ace-step-1.5,ace-step-1.5-xl,ace-step-1.5-xl-sft,ace-step-1.5-sftturbo,ace-step-1.5-clear".into());
    let lyrics = "[Verse]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let music = || media::MusicSpec { genre: Some("Pop".into()), voice: Some("female".into()), instruments: vec!["Piano".into(), "Drums".into(), "Bass".into()], ..Default::default() };
    // (name, model, words added to the description)
    let mut runs: Vec<(String, String, &str)> = Vec::new();
    for m in models.split(',') {
        runs.push((m.to_string(), m.to_string(), ""));
    }
    runs.insert(1, ("ace-step-1.5+upfront".into(), "ace-step-1.5".into(), ", clear lead vocals upfront in the mix, natural human voice"));
    media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let mut rows = Vec::new();
    for (name, model, extra) in &runs {
        media::install_for_test(&state, model).await.unwrap();
        for seed in [11i64, 22] {
            let req = Request {
                op: "music".into(),
                model_id: Some(model.clone()),
                prompt: format!("a hopeful morning song{extra}"),
                lyrics: Some(lyrics.into()),
                seconds: Some(60.0),
                seed: Some(seed),
                music: Some(music()),
                language: Some("en".into()),
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let item = match media::run(&state, store::new_id(), req).await {
                Ok(mut v) => v.remove(0),
                Err(e) => {
                    rows.push(format!("{name} seed {seed}: failed ({e})"));
                    continue;
                }
            };
            let took = t.elapsed().as_secs_f64();
            let bytes = store::read(&state.paths, &c, &item.id).unwrap();
            let file = out.join(format!("{}-{seed}.mp3", name.replace('+', "-")));
            std::fs::write(&file, &bytes).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let words = sung_words(&heard);
            let wanted = sung_words(lyrics);
            let matched = wanted.iter().filter(|w| words.contains(*w)).count();
            let spec = media::MusicSpec { task: Some("extract".into()), audio: Some(item.id.clone()), track: Some("vocals".into()), ..Default::default() };
            let over = match media::run(&state, store::new_id(), Request { op: "music".into(), seconds: Some(60.0), music: Some(spec), ..Default::default() }).await {
                Ok(mut v) => {
                    let (voc, _) = decode_mp3(&store::read(&state.paths, &c, &v.remove(0).id).unwrap());
                    let (mix, _) = decode_mp3(&bytes);
                    crate::audio::rms(&voc) / crate::audio::rms(&mix).max(1e-6)
                }
                Err(_) => f32::NAN,
            };
            rows.push(format!("{name} seed {seed}: {took:.0}s, {matched} of {} lyric words heard, voice to song {over:.2}", wanted.len()));
            println!("{}", rows.last().unwrap());
        }
    }
    state.engine.lock().await.stop().await;
    println!("\n{}\nsongs in {}", rows.join("\n"), out.display());
}

/// Emotional songs, female and male voices, kept for listening, each with
/// the words Whisper hears, its clipped samples and the listener's review;
/// the first also mapped for the song view, its MIDI file kept. (A finish
/// that worked on the separated voice was tested here against the songs as
/// made, and lost when listened to: see `stems`.)
/// SULCUSAI_FINISH_MODEL picks the music model (default: the one this PC is
/// offered), SULCUSAI_FINISH_CASES the songs.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_emotional_songs() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::env::temp_dir().join("sulcusai-finish-ab");
    std::fs::create_dir_all(&out).unwrap();
    let model = std::env::var("SULCUSAI_FINISH_MODEL").ok().unwrap_or_else(|| media::recommended(&state, media::Kind::Music).unwrap().id);
    media::install_for_test(&state, &model).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    // (name, description, voice, feeling, genre, instruments, lyrics)
    let cases: &[(&str, &str, &str, &str, &str, &[&str], &str)] = &[
        ("female-heartbreak", "a heartbroken piano ballad about someone who left", "female", "Heartbroken", "Ballad", &["Piano", "Strings"],
         "[Verse]\nI kept your coat by the door\nLike you'd come back for it\nEvery night I tell the walls\nThat I'm fine, that I'm fine\n\n[Chorus]\nBut I'm falling apart\nIn the place where you were\nSay my name one more time\nSo I know it was real"),
        ("male-heartbreak", "an aching soul ballad, sorry for what he never said", "male", "Heartbroken", "Soul", &["Piano", "Organ", "Bass"],
         "[Verse]\nRain on the windshield, the radio low\nDriving the long way so I don't go home\nYour words in my head like a song on repeat\nI should have held on, I should have held on\n\n[Chorus]\nOh, I'm sorry, I'm sorry\nFor the things I never said\nOh, come back to me\nBefore the morning comes"),
        ("female-anthem", "a powerful pop anthem about getting back up", "female", "Triumphant", "Pop", &["Drums", "Synth", "Bass"],
         "[Verse]\nThey told me I would never fly\nI kept my wings and I kept my fight\nEvery door they closed on me\nTaught me how to break it free\n\n[Chorus]\nI'm still standing, I'm still here\nShout it louder, let them hear\nEvery scar became a song\nI was strong all along"),
        ("male-rock", "a raspy, defiant rock song", "male", "Angry", "Rock", &["Electric guitar", "Drums", "Bass"],
         "[Verse]\nYou built your walls out of my name\nYou lit the fire then called it rain\nI swallowed every word you said\nNow they're burning in my head\n\n[Chorus]\nI won't break, I won't bend\nThis is where your story ends\nTear it down, tear it down\nI'm not afraid of the sound"),
        ("female-tender", "a tender, intimate acoustic love song", "female", "Tender", "Folk", &["Acoustic guitar"],
         "[Verse]\nHold me close while the candle burns\nMorning comes but the world can turn\nYour hand in mine, the quiet room\nNothing else I need but you\n\n[Chorus]\nStay a little longer\nStay till the stars are gone\nYour heartbeat is the only song\nI'll ever need"),
        ("male-longing", "a late-night R&B slow jam full of longing", "male", "Longing", "R&B", &["Electric piano", "Bass", "Drums"],
         "[Verse]\nThree a.m. and I'm wide awake\nCounting all the calls I didn't make\nYour perfume still on the pillow case\nI can't get you out of this place\n\n[Chorus]\nI miss you more than I can say\nEvery night feels the same\nBaby, won't you come my way\nI still call your name"),
    ];
    let pick = std::env::var("SULCUSAI_FINISH_CASES").ok();
    let clipped = |bytes: &[u8]| {
        let (s, _) = decode_mp3(bytes);
        (s.iter().filter(|v| v.abs() >= 0.999).count(), s.iter().fold(0f32, |m, v| m.max(v.abs())))
    };
    let mut rows = Vec::new();
    for (i, (name, prompt, voice, feeling, genre, instruments, lyrics)) in cases.iter().enumerate() {
        if pick.as_deref().is_some_and(|p| !p.split(',').any(|x| x == *name)) {
            continue;
        }
        let music = MusicSpec { genre: Some(genre.to_string()), voice: Some(voice.to_string()), emotion: Some(feeling.to_string()), instruments: instruments.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        let wanted = sung_words(lyrics);
        {
            let how = "song";
            let req = Request {
                op: "music".into(),
                model_id: Some(model.clone()),
                prompt: prompt.to_string(),
                lyrics: Some(lyrics.to_string()),
                seconds: Some(60.0),
                seed: Some(100 + i as i64),
                music: Some(music.clone()),
                language: Some("en".into()),
                ..Default::default()
            };
            let t = std::time::Instant::now();
            let item = match media::run(&state, store::new_id(), req).await {
                Ok(mut v) => v.remove(0),
                Err(e) => {
                    rows.push(format!("{name} {how}: failed ({e})"));
                    continue;
                }
            };
            let took = t.elapsed().as_secs_f64();
            let bytes = store::read(&state.paths, &c, &item.id).unwrap();
            let file = out.join(format!("{name}-{how}.mp3"));
            std::fs::write(&file, &bytes).unwrap();
            let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
            let words = sung_words(&heard);
            let matched = wanted.iter().filter(|w| words.contains(*w)).count();
            let (clips, peak) = clipped(&bytes);
            rows.push(format!("{name} {how}: {took:.0}s, {matched} of {} words heard, {clips} clipped samples (peak {peak:.2}), {} KB", wanted.len(), bytes.len() / 1024));
            println!("{}", rows.last().unwrap());
            // The listener, as the Studio's button runs it.
            {
                let listened = media::run(&state, store::new_id(), Request { op: "listen".into(), source: Some(item.id.clone()), ..Default::default() }).await;
                match listened.map(|mut v| v.remove(0).review) {
                    Ok(Some(r)) => rows.push(format!("  listener: score {}, {} of {} words, unclear {:?}; {}", r.score, r.heard, r.words, r.unclear, r.notes.join(" "))),
                    other => rows.push(format!("  listener failed: {other:?}")),
                }
                println!("{}", rows.last().unwrap());
                // The song view's map of the first song, and its MIDI file.
                if i == 0 {
                    let t = std::time::Instant::now();
                    match media::run(&state, store::new_id(), Request { op: "map".into(), source: Some(item.id.clone()), ..Default::default() }).await.map(|mut v| v.remove(0).map) {
                        Ok(Some(m)) => {
                            let sections: Vec<String> = m.sections.iter().map(|s| format!("{} {:.0}-{:.0}", s.name, s.start, s.end)).collect();
                            let notes: Vec<String> = m.parts.iter().map(|p| format!("{} {}", p.name, p.notes.len())).collect();
                            std::fs::write(out.join(format!("{name}.mid")), crate::media::songmap::midi(&m)).unwrap();
                            rows.push(format!("  map ({:.0}s): sections [{}]; notes [{}]", t.elapsed().as_secs_f64(), sections.join(", "), notes.join(", ")));
                        }
                        other => rows.push(format!("  map failed: {other:?}")),
                    }
                    println!("{}", rows.last().unwrap());
                }
            }
        }
    }
    state.engine.lock().await.stop().await;
    println!("\n{}\nsongs in {}", rows.join("\n"), out.display());
}

/// The listener and its remake on a real song: listened to, its unclear
/// stretches made again, and listened to again. Both kept for listening.
/// SULCUSAI_FINISH_MODEL picks the music model.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_remake() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::env::temp_dir().join("sulcusai-finish-ab");
    std::fs::create_dir_all(&out).unwrap();
    let model = std::env::var("SULCUSAI_FINISH_MODEL").ok().unwrap_or_else(|| media::recommended(&state, media::Kind::Music).unwrap().id);
    media::install_for_test(&state, &model).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let lyrics = "[Verse]\nI kept your coat by the door\nLike you'd come back for it\nEvery night I tell the walls\nThat I'm fine, that I'm fine\n\n[Chorus]\nBut I'm falling apart\nIn the place where you were\nSay my name one more time\nSo I know it was real";
    let req = Request {
        op: "music".into(),
        model_id: Some(model.clone()),
        prompt: "a heartbroken piano ballad about someone who left".into(),
        lyrics: Some(lyrics.into()),
        seconds: Some(60.0),
        seed: Some(100),
        music: Some(MusicSpec { genre: Some("Ballad".into()), voice: Some("female".into()), emotion: Some("Heartbroken".into()), instruments: vec!["Piano".into(), "Strings".into()], ..Default::default() }),
        language: Some("en".into()),
        ..Default::default()
    };
    let song = media::run(&state, store::new_id(), req).await.unwrap().remove(0);
    let listened = media::run(&state, store::new_id(), Request { op: "listen".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
    let before = listened.review.clone().unwrap();
    println!("before: score {}, {} of {} words, unclear {:?}", before.score, before.heard, before.words, before.unclear);
    std::fs::write(out.join("remake-1-before.mp3"), store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    if before.unclear.is_empty() {
        println!("every part was clear: the remake has nothing to do");
    }
    // A section changed from the song view: the new version comes back mapped.
    let t = std::time::Instant::now();
    let edit = MusicSpec { task: Some("repaint".into()), audio: Some(song.id.clone()), region: Some([20.0, 35.0]), ..Default::default() };
    let edited = media::run(
        &state,
        store::new_id(),
        Request { op: "music".into(), model_id: Some(model.clone()), prompt: format!("{}, bigger with strings", song.prompt), lyrics: Some(lyrics.into()), music: Some(edit), then_map: Some(true), ..Default::default() },
    )
    .await
    .unwrap()
    .remove(0);
    let m = edited.map.as_ref().expect("the edited version is mapped");
    println!(
        "section edit ({:.0}s): sections [{}]; notes [{}]",
        t.elapsed().as_secs_f64(),
        m.sections.iter().map(|s| format!("{} {:.0}-{:.0}", s.name, s.start, s.end)).collect::<Vec<_>>().join(", "),
        m.parts.iter().map(|p| format!("{} {}", p.name, p.notes.len())).collect::<Vec<_>>().join(", ")
    );
    std::fs::write(out.join("remake-3-section-edit.mp3"), store::read(&state.paths, &c, &edited.id).unwrap()).unwrap();
    if before.unclear.is_empty() {
        state.engine.lock().await.stop().await;
        return;
    }
    let t = std::time::Instant::now();
    let remade = media::run(&state, store::new_id(), Request { op: "remake".into(), source: Some(song.id.clone()), model_id: Some(model.clone()), prompt: song.prompt.clone(), ..Default::default() }).await;
    state.engine.lock().await.stop().await;
    let remade = remade.unwrap().remove(0);
    let after = remade.review.clone().unwrap();
    std::fs::write(out.join("remake-2-after.mp3"), store::read(&state.paths, &c, &remade.id).unwrap()).unwrap();
    println!(
        "after ({:.0}s): score {}, {} of {} words, unclear {:?}; note {:?}; parent kept: {}",
        t.elapsed().as_secs_f64(),
        after.score,
        after.heard,
        after.words,
        after.unclear,
        remade.note,
        store::get(&state.db.lock().unwrap(), &c, &song.id).is_some()
    );
    assert_eq!(remade.parent.as_deref(), Some(song.id.as_str()));
}

/// Songs made the way the Studio and chats make them, with the real music
/// engine: lyrics written by the chat model (English and Spanish), the
/// person's own lyrics, an instrumental, a song with no chat model to write
/// its lyrics, a sound effect, and a music model installed from a pasted
/// Hugging Face link (its own file deleted afterwards). Each is checked for
/// length and sound, and the sung ones by speech recognition (Whisper large
/// v3 turbo, installed the way the app installs it): the words it hears
/// against the lyrics. SULCUSAI_SONGS picks cases (default all);
/// SULCUSAI_MEDIA_OUT keeps the MP3s; SULCUSAI_LYRICS_MODEL is the chat
/// model (default qwen3-4b-2507); SULCUSAI_HUB_SONG the link.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_music_songs() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-music-out").display().to_string()));
    std::fs::create_dir_all(&out).unwrap();
    let cases = std::env::var("SULCUSAI_SONGS").unwrap_or_else(|_| "sound,instrumental,no_chat_model,written,spanish,own,shaped,long,cover,repaint,extend,parts,hub".into());
    let want = |p: &str| cases.split(',').any(|x| x == p);
    let started = std::time::Instant::now();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    println!("installed ACE-Step 1.5 in {:.0}s", started.elapsed().as_secs_f64());

    // (name, what was asked, item made or why not, language to listen for)
    let mut made: Vec<(String, Request, Result<media::MediaItem, String>, Option<&str>)> = Vec::new();
    let make = |name: &str, req: Request| {
        let state = state.clone();
        let name = name.to_string();
        async move {
            let t = std::time::Instant::now();
            let r = media::run(&state, store::new_id(), req).await.map(|mut v| v.remove(0));
            println!("{name}: {:.0}s -> {}", t.elapsed().as_secs_f64(), match &r {
                Ok(i) => format!("{:.1}s, lyrics {:?}", i.seconds, i.lyrics.as_deref().map(|l| l.chars().take(120).collect::<String>())),
                Err(e) => e.clone(),
            });
            r
        }
    };
    let folk = "upbeat acoustic folk song about a road trip through the mountains, warm male vocals, guitar and harmonica";
    if want("sound") {
        let req = Request { op: "sound".into(), prompt: "rain on a tin roof with distant thunder".into(), seconds: Some(10.0), ..Default::default() };
        made.push(("sound".into(), req.clone(), make("sound", req).await, None));
    }
    if want("instrumental") {
        let req = Request { op: "music".into(), prompt: "mellow lofi hip hop beat with soft piano and vinyl crackle".into(), seconds: Some(30.0), instrumental: Some(true), ..Default::default() };
        made.push(("instrumental".into(), req.clone(), make("instrumental", req).await, None));
    }
    // No chat model on this PC yet: the music model writes the lyrics itself.
    if want("no_chat_model") {
        let req = Request { op: "music".into(), prompt: "anthemic rock song about old friends, powerful male vocals, electric guitars and drums".into(), seconds: Some(30.0), ..Default::default() };
        made.push(("no_chat_model".into(), req.clone(), make("no_chat_model", req).await, Some("en")));
    }
    // A chat model, as the default (not loaded yet): it writes the lyrics.
    lyrics_model(&state);
    if want("written") {
        let req = Request { op: "music".into(), prompt: folk.into(), seconds: Some(30.0), ..Default::default() };
        made.push(("written".into(), req.clone(), make("written", req).await, Some("en")));
    }
    if want("spanish") {
        let req = Request { op: "music".into(), prompt: "energetic latin pop song about summer nights by the sea, bright female vocals".into(), seconds: Some(30.0), language: Some("es".into()), ..Default::default() };
        made.push(("spanish".into(), req.clone(), make("spanish", req).await, Some("es")));
    }
    if want("own") {
        let lyrics = "[Verse]\nMorning light across the kitchen floor\nCoffee waiting by the open door\nEvery little thing I need is here\nSinging softly so the world can hear\n\n[Chorus]\nHold on, hold on to the sunrise\nHold on, never say goodbye";
        let req = Request { op: "music".into(), prompt: "gentle pop ballad, soft female vocals, piano and strings".into(), seconds: Some(60.0), lyrics: Some(lyrics.into()), language: Some("en".into()), ..Default::default() };
        made.push(("own".into(), req.clone(), make("own", req).await, Some("en")));
    }
    // Shaped in the Studio: style, mood, instruments, voice, beat, tempo, key and time signature.
    if want("shaped") {
        let music = media::MusicSpec {
            // Synthwave's vocals come out blurred (a trait of the style here): pop, to hear the words.
            genre: Some("Synth pop".into()),
            mood: Some("Nostalgic".into()),
            instruments: vec!["Synth".into(), "Drums".into(), "Electric guitar".into()],
            voice: Some("female".into()),
            beat: Some("Four on the floor".into()),
            bpm: Some(118),
            key: Some("A minor".into()),
            time_signature: Some("4/4".into()),
            ..Default::default()
        };
        let req = Request { op: "music".into(), prompt: "driving through a neon city at night".into(), seconds: Some(40.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("shaped".into(), req.clone(), make("shaped", req).await, Some("en")));
    }
    // Two versions at once: two songs, each as long as asked.
    if want("versions") {
        let req = Request { op: "music".into(), prompt: "cheerful ukulele pop about a sunny weekend, bright female vocals".into(), seconds: Some(30.0), count: Some(2), ..Default::default() };
        let t = std::time::Instant::now();
        let r = media::run(&state, store::new_id(), req.clone()).await;
        println!("versions: {:.0}s -> {:?}", t.elapsed().as_secs_f64(), r.as_ref().map(|v| v.iter().map(|i| i.seconds).collect::<Vec<_>>()));
        let v = r.unwrap();
        assert_eq!(v.len(), 2, "two versions");
        for (k, i) in v.into_iter().enumerate() {
            made.push((format!("version{}", k + 1), req.clone(), Ok(i), Some("en")));
        }
    }
    // A long song: as long as asked.
    if want("long") {
        let req = Request { op: "music".into(), prompt: "slow soulful blues ballad about a long road home, raspy male vocals, electric guitar and organ".into(), seconds: Some(150.0), ..Default::default() };
        made.push(("long".into(), req.clone(), make("long", req).await, Some("en")));
    }
    // Rap, the way the Studio asks for it: hip hop with a rap voice.
    if want("rap") {
        let music = media::MusicSpec { genre: Some("Hip hop".into()), voice: Some("rap".into()), ..Default::default() };
        let req = Request { op: "music".into(), prompt: "rap about grinding at a night job to pay for college".into(), seconds: Some(60.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("rap".into(), req.clone(), make("rap", req).await, Some("en")));
    }
    // An intro in the structure: singing should start with it, not after a long instrumental.
    if want("intro") {
        let music = media::MusicSpec { genre: Some("Pop".into()), voice: Some("female".into()), sections: ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"].map(String::from).to_vec(), ..Default::default() };
        let req = Request { op: "music".into(), prompt: "driving home at sunrise after a long night".into(), seconds: Some(150.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("intro".into(), req.clone(), make("intro", req).await, Some("en")));
    }
    // The same, told the singing starts right away: does the lead-in shorten?
    if want("intro_soon") {
        let music = media::MusicSpec { genre: Some("Pop".into()), voice: Some("female".into()), sections: ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"].map(String::from).to_vec(), ..Default::default() };
        let req = Request { op: "music".into(), prompt: "driving home at sunrise after a long night, short intro, the singing starts right away".into(), seconds: Some(150.0), music: Some(music), language: Some("en".into()), ..Default::default() };
        made.push(("intro_soon".into(), req.clone(), make("intro_soon", req).await, Some("en")));
    }
    // Auto length: a jingle comes out short, a full song a few minutes, as the lyrics need.
    for (case, prompt) in [("auto_jingle", "a cheerful little jingle for a neighborhood bakery"), ("auto_full", "a heartfelt pop song about leaving your hometown for the big city")] {
        if want(case) {
            let music = media::MusicSpec { genre: Some("Pop".into()), auto_length: true, ..Default::default() };
            let req = Request { op: "music".into(), prompt: prompt.into(), music: Some(music), language: Some("en".into()), ..Default::default() };
            let r = make(case, req.clone()).await;
            // Checked against the length the song came out at (Auto sets it).
            let asked = r.as_ref().map_or(30.0, |i| i.seconds as f32);
            made.push((case.into(), Request { seconds: Some(asked), ..req }, r, Some("en")));
        }
    }
    // A saved artist: the first song becomes the voice, the second follows
    // it; a third with the same settings and no artist is the control.
    if want("artist_voice") {
        let sound = media::MusicSpec { genre: Some("Indie pop".into()), voice: Some("female".into()), vocal_style: Some("breathy".into()), instruments: vec!["Piano".into(), "Acoustic guitar".into()], ..Default::default() };
        let nova = {
            let conn = state.db.lock().unwrap();
            media::persona::save(&conn, &c, media::persona::Persona { name: "Nova".into(), sound: sound.clone(), language: Some("en".into()), ..Default::default() }).unwrap()
        };
        let with = |prompt: &str, persona: bool| Request {
            op: "music".into(),
            prompt: prompt.into(),
            seconds: Some(60.0),
            music: Some(if persona { media::MusicSpec { persona: Some("Nova".into()), ..Default::default() } } else { sound.clone() }),
            language: Some("en".into()),
            ..Default::default()
        };
        let first = with("city lights on a quiet night", true);
        made.push(("artist_first".into(), first.clone(), make("artist_first", first).await, Some("en")));
        let voice = media::persona::find(&state.db.lock().unwrap(), &c, &nova.id).and_then(|p| p.voice_song);
        println!("Nova's voice song: {voice:?}");
        let second = with("a road trip with my best friend", true);
        made.push(("artist_second".into(), second.clone(), make("artist_second", second).await, Some("en")));
        let control = with("a road trip with my best friend", false);
        made.push(("artist_control".into(), control.clone(), make("artist_control", control).await, Some("en")));
    }
    // Many styles, one each (SULCUSAI_SONGS=genres): words heard in every one.
    if want("genres") {
        let styles: [(&str, &str, Option<&str>, &str, Option<&str>); 10] = [
            ("trap", "Trap", Some("rap"), "counting money and ignoring the haters", None),
            ("country", "Country", Some("male"), "a small town Friday night", None),
            ("rock", "Rock", Some("male"), "breaking free from a dead-end job", None),
            ("metal", "Metal", Some("male"), "a storm tearing through the mountains", None),
            ("rnb", "R&B", Some("female"), "falling for someone slowly", None),
            ("reggaeton", "Reggaeton", Some("male"), "una fiesta en la playa", Some("es")),
            ("kpop", "K-pop", Some("female"), "a confident summer dance anthem", None),
            ("gospel", "Gospel", Some("choir"), "hope after hard times", None),
            ("jazz", "Jazz", Some("female"), "a rainy night in a city cafe", None),
            ("folk", "Folk", Some("duet"), "two old friends meeting again", None),
        ];
        for (name, genre, voice, prompt, lang) in styles {
            let music = media::MusicSpec { genre: Some(genre.into()), voice: voice.map(str::to_string), ..Default::default() };
            let req = Request { op: "music".into(), prompt: prompt.into(), seconds: Some(60.0), music: Some(music), language: Some(lang.unwrap_or("en").into()), ..Default::default() };
            made.push((name.into(), req.clone(), make(name, req).await, Some(lang.unwrap_or("en"))));
        }
    }
    // A minute's song with its lyrics written: a verse and a chorus (it was three lines).
    if want("minute") {
        let req = Request { op: "music".into(), prompt: folk.into(), seconds: Some(60.0), ..Default::default() };
        made.push(("minute".into(), req.clone(), make("minute", req).await, Some("en")));
    }
    // In an artist's style, asked in the description.
    if want("artist") {
        let req = Request { op: "music".into(), prompt: "Sing as Taylor Swift a song about bears".into(), seconds: Some(60.0), ..Default::default() };
        made.push(("artist".into(), req.clone(), make("artist", req).await, Some("en")));
    }
    // Shaped in depth: sections, rhyme, a key, a feeling, sung harmonies and ways of singing.
    let deep = |layer: bool| media::MusicSpec {
        genre: Some("Pop rock".into()),
        voice: Some("male".into()),
        emotion: Some("Heartbroken".into()),
        key: Some("D major".into()),
        sections: ["Verse", "Chorus", "Verse", "Chorus"].map(String::from).to_vec(),
        rhyme: Some("abab".into()),
        harmony_voice: Some("female".into()),
        harmony_where: Some("chorus".into()),
        harmony_instrument: layer.then(|| "Strings".into()),
        harmony_layer: layer,
        vocal_effects: vec![media::VocalEffect { how: "vibrato".into(), on: "all".into() }, media::VocalEffect { how: "belting".into(), on: "chorus".into() }],
        ..Default::default()
    };
    if want("deep") {
        let req = Request { op: "music".into(), prompt: "the last night before you moved away".into(), seconds: Some(120.0), music: Some(deep(false)), language: Some("en".into()), ..Default::default() };
        made.push(("deep".into(), req.clone(), make("deep", req).await, Some("en")));
    }
    // The harmonies layered on afterwards by ACE-Step 1.5 Base.
    if want("layered") {
        media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
        let req = Request { op: "music".into(), prompt: "the last night before you moved away".into(), seconds: Some(60.0), music: Some(deep(true)), language: Some("en".into()), ..Default::default() };
        made.push(("layered".into(), req.clone(), make("layered", req).await, Some("en")));
    }
    // Work on a song: the written one (or the first song made), the
    // instrumental for adding a part.
    let first_song = made.iter().find(|m| m.0 == "written").or_else(|| made.iter().find(|m| m.3.is_some())).and_then(|m| m.2.as_ref().ok()).cloned();
    let work = |task: &str, song: &media::MediaItem, extra: media::MusicSpec| media::MusicSpec { task: Some(task.into()), audio: Some(song.id.clone()), ..extra };
    if let (true, Some(song)) = (want("cover"), first_song.as_ref()) {
        let spec = work("cover", song, media::MusicSpec { genre: Some("Jazz".into()), instruments: vec!["Piano".into(), "Saxophone".into()], strength: Some(0.5), ..Default::default() });
        let req = Request { op: "music".into(), prompt: "as a smoky late-night jazz club song".into(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
        made.push(("cover".into(), req.clone(), make("cover", req).await, Some("en")));
    }
    if let (true, Some(song)) = (want("repaint"), first_song.as_ref()) {
        let spec = work("repaint", song, media::MusicSpec { region: Some([10.0, 20.0]), ..Default::default() });
        let req = Request { op: "music".into(), prompt: "a soaring guitar solo".into(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
        made.push(("repaint".into(), req.clone(), make("repaint", req).await, Some("en")));
    }
    if let (true, Some(song)) = (want("extend"), first_song.as_ref()) {
        let spec = work("extend", song, media::MusicSpec { extend_by: Some(20.0), ..Default::default() });
        let req = Request { op: "music".into(), prompt: String::new(), seconds: Some(song.seconds as f32 + 20.0), music: Some(spec), ..Default::default() };
        made.push(("extend".into(), req.clone(), make("extend", req).await, Some("en")));
    }
    // Parts need ACE-Step 1.5 Base: installed for these.
    if want("parts") {
        media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
        if let Some(song) = first_song.as_ref() {
            let spec = work("extract", song, media::MusicSpec { track: Some("vocals".into()), ..Default::default() });
            let req = Request { op: "music".into(), prompt: String::new(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
            made.push(("extract_vocals".into(), req.clone(), make("extract_vocals", req).await, None));
        }
        let beat = made.iter().find(|m| m.0 == "instrumental").and_then(|m| m.2.as_ref().ok()).cloned();
        if let Some(song) = beat {
            let spec = work("add", &song, media::MusicSpec { track: Some("drums".into()), ..Default::default() });
            let req = Request { op: "music".into(), prompt: "punchy live drums".into(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
            made.push(("add_drums".into(), req.clone(), make("add_drums", req).await, None));
        }
    }
    // A music model from a pasted Hugging Face link, sharing the catalog model's files.
    let mut hub_file = None;
    if want("hub") {
        let link = std::env::var("SULCUSAI_HUB_SONG").unwrap_or_else(|_| "https://huggingface.co/Serveurperso/ACE-Step-1.5-GGUF/blob/main/acestep-v15-xl-turbo-Q4_K_M.gguf".into());
        let (repo, file) = crate::hub::parse_link(&link).unwrap();
        let client = crate::hub::client(&state).unwrap();
        let r = crate::hub::repo(&client, &repo).await.unwrap();
        let (found, kind) = media::hub::from_link(&state, &r).expect("a music model SulcusAI runs");
        assert_eq!(kind, "music");
        let file = file.expect("the link names a file");
        let offer = found.files.iter().find(|f| f.file == file).expect("the linked file is offered");
        println!("hub: {} {} ({:.2} GB, {:.2} GB to download), runs: {}", found.name, offer.quant, offer.size as f64 / 1e9, offer.download as f64 / 1e9, offer.runnable);
        let spec = media::hub::prepare(&state, &repo, &file).await.unwrap();
        println!("hub files: {:?}", spec.files.iter().map(|f| (&f.role, &f.file, f.size)).collect::<Vec<_>>());
        let t = std::time::Instant::now();
        media::install_for_test(&state, &spec.id).await.unwrap();
        println!("installed {} in {:.0}s", spec.name, t.elapsed().as_secs_f64());
        hub_file = spec.files.iter().find(|f| f.file.starts_with("hfm-")).map(|f| media::model_dir(&state.paths, &spec).join(&f.file));
        let req = Request { op: "music".into(), model_id: Some(spec.id.clone()), prompt: folk.into(), seconds: Some(30.0), ..Default::default() };
        let r = make("hub", req.clone()).await;
        if let Ok(i) = &r {
            assert_eq!(i.model_id.as_deref(), Some(spec.id.as_str()), "made with the Hugging Face model");
        }
        made.push(("hub".into(), req, r, Some("en")));
    }
    state.engine.lock().await.stop().await;

    // Where the voice is (SULCUSAI_VOCALS=1): ACE-Step 1.5 Base takes the
    // vocals out of each sung song, and their loudness every 3 seconds shows
    // when the singing starts (Whisper hears in 30-second windows, too
    // coarse for a long intro).
    if std::env::var("SULCUSAI_VOCALS").is_ok() {
        media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
        for (name, _, r, lang) in &made {
            let (Ok(song), Some(_)) = (r, lang) else { continue };
            let spec = media::MusicSpec { task: Some("extract".into()), audio: Some(song.id.clone()), track: Some("vocals".into()), ..Default::default() };
            let req = Request { op: "music".into(), prompt: String::new(), seconds: Some(song.seconds as f32), music: Some(spec), ..Default::default() };
            let Ok(mut v) = media::run(&state, store::new_id(), req).await else { continue };
            let (samples, rate) = decode_mp3(&store::read(&state.paths, &c, &v.remove(0).id).unwrap());
            let step = rate as usize * 3;
            let levels: Vec<f32> = samples.chunks(step).map(crate::audio::rms).collect();
            let loudest = levels.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
            let line: String = levels.iter().map(|l| match l / loudest { x if x > 0.5 => '#', x if x > 0.2 => '+', x if x > 0.08 => '.', _ => ' ' }).collect();
            let first = levels.iter().position(|l| l / loudest > 0.2).map(|i| i * 3);
            let (pitch, bright) = voice_print(&samples, rate);
            // How loud the voice is against the whole song (drowned out: low).
            let (mix, _) = decode_mp3(&store::read(&state.paths, &c, &song.id).unwrap());
            let over = crate::audio::rms(&samples) / crate::audio::rms(&mix).max(1e-6);
            println!(
                "{name}: vocals every 3 s |{line}| first at {}, voice {:.0} Hz, brightness {:.3}, voice to song {:.2}",
                first.map_or("never".into(), |s| format!("{s}s")),
                pitch,
                bright,
                over
            );
        }
        state.engine.lock().await.stop().await;
    }

    // Listening: length, sound, and the words speech recognition hears.
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let mut failed = Vec::new();
    for (name, req, r, lang) in &made {
        let item = match r {
            Ok(i) => i,
            // No chat model to write lyrics: said plainly, not sung as made-up
            // words (the music model's own lyrics came out as garbage).
            Err(e) if name == "no_chat_model" && e.contains("lyrics") => {
                println!("{name}: said why, as it should: {e}");
                continue;
            }
            Err(e) => {
                failed.push(format!("{name}: {e}"));
                continue;
            }
        };
        let bytes = store::read(&state.paths, &c, &item.id).unwrap();
        std::fs::write(out.join(format!("{name}.mp3")), &bytes).unwrap();
        let (samples, rate) = decode_mp3(&bytes);
        let secs = samples.len() as f64 / rate as f64;
        let rms = crate::audio::rms(&samples);
        let asked = req.seconds.unwrap_or(30.0) as f64;
        // Whisper without the voice detector the app's server uses: the
        // detector took singing over music for no speech at all.
        let (heard, starts) = heard_words(&whisper_hears(&state, &out.join(format!("{name}.mp3")), *lang));
        let words = sung_words(&heard);
        let lyrics = item.lyrics.clone().unwrap_or_default();
        let wanted = sung_words(&lyrics);
        let matched = wanted.iter().filter(|w| words.contains(*w)).count();
        // Words a second heard: rap is dense, a ballad sparse.
        let spoken = heard.split_whitespace().count();
        println!(
            "{name}: {secs:.1}s (asked {asked:.0}s), level {rms:.3}, {} words heard, {matched} of {} lyric words, first words at {}, {spoken} words ({:.1} a second)\n  heard: {}\n  lyrics: {}",
            words.len(),
            wanted.len(),
            starts.map_or("never".to_string(), |s| format!("{s:.0}s")),
            spoken as f64 / secs.max(1.0),
            heard.chars().take(400).collect::<String>(),
            lyrics.replace('\n', " / ").chars().take(500).collect::<String>()
        );
        if (secs - asked).abs() > 3.0 {
            failed.push(format!("{name}: {secs:.1}s long, asked for {asked:.0}s"));
        }
        if rms < 0.01 {
            failed.push(format!("{name}: nearly silent ({rms:.4})"));
        }
        let sung = lang.is_some();
        if sung && (wanted.is_empty() || matched * 5 < wanted.len()) {
            failed.push(format!("{name}: few of the lyrics were heard ({matched} of {})", wanted.len()));
        }
        if !sung && item.lyrics.is_some() {
            failed.push(format!("{name}: has lyrics but shouldn't be sung"));
        }
        // Written lyrics fill the song (a minute had three lines).
        let lines = crate::media::lyrics::sung_lines(&lyrics);
        if sung && req.lyrics.is_none() && asked >= 60.0 && (lines as f64) < asked / crate::media::lyrics::SECS_PER_LINE as f64 * 0.7 {
            failed.push(format!("{name}: only {lines} lines for {asked:.0}s"));
        }
    }
    if let Some(p) = hub_file {
        std::fs::remove_file(&p).ok();
    }
    println!("results in {}", out.display());
    assert!(failed.is_empty(), "{failed:#?}");
}

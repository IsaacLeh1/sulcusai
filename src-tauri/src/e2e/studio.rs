// SPDX-License-Identifier: AGPL-3.0-only
//! The song studio and Compose: parts, effects, custom models, covers.

use super::*;

/// The Suno-style song features through the app's own jobs: best of 2 (the
/// clearest kept, with its review), the hook as the title, the settings kept
/// for Reuse, cover art from Z-Image Turbo, and the stems as songs of their
/// own. The song, its art and its parts go to C:\dev\tools\music\ab\features.
///     cargo test e2e_suno_features -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_suno_features() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    for model in ["ace-step-1.5-xl", "z-image-turbo"] {
        media::install_for_test(&state, model).await.unwrap_or_else(|e| panic!("installing {model}: {e}"));
    }
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab\features");
    std::fs::create_dir_all(&out).unwrap();
    let lyrics = "[Verse 1]\nWindows down on the coast road\nRadio loud and the night is young\n\n[Chorus]\nSummer nights, we were golden\nDancing under the neon sun\nSummer nights, we were golden\nNever wanted the night to be done";
    let t = std::time::Instant::now();
    let req = Request {
        op: "music".into(),
        model_id: Some("ace-step-1.5-xl".into()),
        prompt: "upbeat 80s-inspired synth pop about a summer drive".into(),
        lyrics: Some(lyrics.into()),
        seconds: Some(45.0),
        seed: Some(5),
        count: Some(2),
        language: Some("en".into()),
        music: Some(MusicSpec { voice: Some("female".into()), best_of: true, exclude: Some("saxophone".into()), ..Default::default() }),
        ..Default::default()
    };
    let made = media::run(&state, store::new_id(), req).await.unwrap();
    println!("best of 2 in {:.0} s: {} kept, note {:?}, title {:?}", t.elapsed().as_secs_f64(), made.len(), made[0].note, made[0].title);
    assert_eq!(made.len(), 1, "only the clearest is kept");
    let song = made[0].clone();
    assert_eq!(song.title.as_deref(), Some("Summer Nights"));
    assert!(song.review.is_some() && song.note.as_deref().is_some_and(|n| n.contains("clearest of 2")));
    assert_eq!(song.music.as_ref().and_then(|m| m.exclude.as_deref()), Some("saxophone"), "its settings are kept for Reuse");
    std::fs::write(out.join("song.mp3"), store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    let t = std::time::Instant::now();
    let art = media::run(&state, store::new_id(), Request { op: "cover_art".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
    assert!(art.poster && art.id == song.id);
    let jpeg = store::read_thumb(&state.paths, &c, &song.id).unwrap();
    let img = image::load_from_memory(&jpeg).unwrap();
    std::fs::write(out.join("cover.jpg"), &jpeg).unwrap();
    println!("cover art in {:.0} s: {}x{}", t.elapsed().as_secs_f64(), img.width(), img.height());
    assert_eq!(img.width(), img.height());
    assert_eq!(store::list(&state.db.lock().unwrap(), &c, Some("image"), 50, None).len(), 0, "the picture itself isn't kept");
    let t = std::time::Instant::now();
    let parts = media::run(&state, store::new_id(), Request { op: "stems".into(), source: Some(song.id.clone()), music: Some(MusicSpec { track: Some("all".into()), ..Default::default() }), ..Default::default() }).await.unwrap();
    println!("stems in {:.0} s: {:?}", t.elapsed().as_secs_f64(), parts.iter().map(|p| (p.title.clone().unwrap_or_default(), p.seconds.round())).collect::<Vec<_>>());
    assert_eq!(parts.len(), 4);
    for p in &parts {
        assert_eq!(p.parent.as_deref(), Some(song.id.as_str()));
        assert!((p.seconds - song.seconds).abs() < 1.0);
        std::fs::write(out.join(format!("{}.wav", p.title.clone().unwrap_or_default())), store::read(&state.paths, &c, &p.id).unwrap()).unwrap();
    }
}

/// Remove FX through the app's own job: a dry voice with a room's reverb
/// (and with an echo too) made dry again, on the NVIDIA card when there is
/// one. The test sounds are C:\dev\tools\music\dereverb\test (scripts/wet.py
/// makes and scores them); the cleaned ones are written beside them. The
/// model is copied from the export there when it isn't in the models
/// folder yet (it's the file on the engines release).
///     cargo test e2e_remove_fx -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_remove_fx() {
    use crate::media::{self, store, MediaItem, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let dir = std::path::PathBuf::from(r"C:\dev\tools\music\dereverb");
    let h = state.catalog.media.dereverb.clone().unwrap();
    let dest = state.paths.models.join("media").join("helpers").join(&h.file.file);
    if !dest.metadata().is_ok_and(|m| m.len() == h.file.size) {
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(dir.join("out").join(&h.file.file), &dest).unwrap();
    }
    for name in ["reverb", "reverb-echo"] {
        let bytes = std::fs::read(dir.join("test").join(format!("{name}.wav"))).unwrap();
        let item = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "audio".into(), op: "import".into(), mime: "audio/wav".into(), seconds: 14.8, ..Default::default() }, &bytes, None).unwrap();
        let t = std::time::Instant::now();
        let got = media::run(&state, store::new_id(), Request { op: "dry".into(), source: Some(item.id.clone()), ..Default::default() }).await.unwrap();
        println!("{name}: dry in {:.1} s, {:.1} s long, hidden {}", t.elapsed().as_secs_f64(), got[0].seconds, got[0].hidden);
        assert!(got[0].hidden && (got[0].seconds - 14.8).abs() < 0.2);
        std::fs::write(dir.join("test").join(format!("{name}-dry.wav")), store::read(&state.paths, &c, &got[0].id).unwrap()).unwrap();
    }
}

/// The song studio's chat bar with a real chat model: a few requests on a
/// two-track project, and the actions it turns them into.
///     cargo test e2e_studio_ask -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_studio_ask() {
    let state = media_state();
    lyrics_model(&state);
    let summary = "Tempo 120 bpm, 4 beats a bar. Length 3:00. Playhead 0:00.\nTracks:\n1. \"Song\" (audio) volume 0 dB, pan 0, effects: none; clips: 0-180 s\n2. \"Keys\" (midi, instrument 0) volume 0 dB, pan 0, effects: none; clips: 10-40 s";
    let mut ok = 0;
    let asks = [
        ("add some reverb to the keys and make the song 95 bpm", &["add_effect", "tempo"][..]),
        ("mute the song track and turn the keys down 6 dB", &["mute", "volume"][..]),
        ("make a funky bass part for the first minute", &["make_part"][..]),
        ("loop from 30 to 45 seconds", &["loop"][..]),
        ("put the keys on the synth as a gritty reese bass with a slow wobble", &["synth"][..]),
        ("build me a lo-fi telephone voice effect and put it on the song", &["build_effect"][..]),
    ];
    for (ask, want) in asks {
        let t = std::time::Instant::now();
        let got = crate::media::ask_studio(&state, ask, summary).await;
        let kinds: Vec<String> = got.as_ref().map(|a| a.iter().map(|x| x["do"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
        let hit = want.iter().all(|w| kinds.iter().any(|k| k == w));
        ok += hit as usize;
        println!("{ask:?} in {:.1} s -> {:?} {}", t.elapsed().as_secs_f64(), got.map(|a| serde_json::to_string(&a).unwrap()), if hit { "ok" } else { "MISSED" });
    }
    println!("{ok} of {} understood", asks.len());
    assert!(ok >= 5);
}

/// The song studio's new part: a guitar added to the synth-pop song from
/// e2e_suno_features by ACE-Step Base and taken out alone (one take), its
/// length and loudness against the mix; then its notes read (Basic Pitch).
/// The part is kept in C:\dev\tools\music\ab\studio-part.wav.
///     cargo test e2e_studio_part -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_studio_part() {
    use crate::media::{self, store, MediaItem, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5-base").await.unwrap();
    let mp3 = std::fs::read(r"C:\dev\tools\music\ab\features\song.mp3").expect("run e2e_suno_features first");
    let mix = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "audio".into(), op: "studio".into(), prompt: "mix".into(), mime: "audio/mpeg".into(), seconds: 45.0, hidden: true, ..Default::default() }, &mp3, None).unwrap();
    let t = std::time::Instant::now();
    let parts = media::run(&state, store::new_id(), Request { op: "studio_part".into(), prompt: "bright funky rhythm guitar".into(), source: Some(mix.id.clone()), count: Some(1), music: Some(MusicSpec { track: Some("guitar".into()), ..Default::default() }), ..Default::default() }).await.unwrap();
    let part = &parts[0];
    let wav = store::read(&state.paths, &c, &part.id).unwrap();
    let file = std::path::PathBuf::from(r"C:\dev\tools\music\ab\studio-part").with_extension(if part.mime.contains("wav") { "wav" } else { "mp3" });
    std::fs::write(&file, &wav).unwrap();
    let (samples, _, _) = crate::media::audio_in_decode_for_test(&file);
    let (mix_samples, _, _) = crate::media::audio_in_decode_for_test(std::path::Path::new(r"C:\dev\tools\music\ab\features\song.mp3"));
    let db = |x: &[f32]| 20.0 * crate::audio::rms(x).max(1e-9).log10();
    println!("part in {:.0} s: {:.0} s long ({:.0} s mix), {:.1} dB against the mix's {:.1} dB, hidden {}, title {:?}", t.elapsed().as_secs_f64(), part.seconds, mix.seconds, db(&samples), db(&mix_samples), part.hidden, part.title);
    assert!(part.hidden && (part.seconds - 45.0).abs() < 3.0 && db(&samples) > -45.0);
    assert!(store::get(&state.db.lock().unwrap(), &c, &parts[0].id).is_some());
    let t = std::time::Instant::now();
    let notes = media::studio_midi_for_test(&state, &part.id, false).await.unwrap();
    println!("{} notes read in {:.0} s", notes.len(), t.elapsed().as_secs_f64());
    assert!(!notes.is_empty());
}

/// A custom model trained the Studio's way on six songs (the test songs in
/// C:\dev\tools\music\train-test\audio): the trainer set up from the
/// catalog (uv and its packages; the source from the release zip in
/// C:\dev\sulcusai-dist\music, unpacked as the app would), the songs'
/// words heard, SULCUSAI_TRAIN_EPOCHS passes (default 30), then a song made
/// with it on ACE-Step 1.5 Base. Kept in C:\dev\tools\music\ab\custom.mp3.
///     cargo test e2e_custom_model -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_custom_model() {
    use crate::media::{self, store, train, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let t = state.catalog.media.trainer.clone().unwrap();
    let source = train::source_dir(&state.paths, &t);
    if !source.join("train.py").is_file() {
        let zip = std::path::PathBuf::from(r"C:\dev\sulcusai-dist\music").join(format!("ace-step-1.5-trainer-{}.zip", t.build));
        let mut a = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        a.extract(&source).unwrap();
    }
    if std::env::var("SULCUSAI_TRAIN_EPOCHS").is_err() {
        std::env::set_var("SULCUSAI_TRAIN_EPOCHS", "30");
    }
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    media::install_for_test(&state, train::BASE_MODEL).await.unwrap();
    let files: Vec<String> = std::fs::read_dir(r"C:\dev\tools\music\train-test\audio").unwrap().flatten().map(|e| e.path().to_string_lossy().into_owned()).collect();
    let t0 = std::time::Instant::now();
    let req = Request { op: "train".into(), prompt: "Test songs".into(), train: Some(train::TrainSpec { name: "Test songs".into(), description: "modern country, warm male lead vocal, acoustic guitar, pedal steel".into(), files, songs: Vec::new(), voice: None }), ..Default::default() };
    media::run(&state, store::new_id(), req).await.unwrap();
    let models = train::list(&state.db.lock().unwrap(), &c);
    println!("trained in {:.0} s: {:?}", t0.elapsed().as_secs_f64(), models.iter().map(|m| (&m.name, &m.trigger, m.songs, m.epochs)).collect::<Vec<_>>());
    let cm = models.first().expect("a custom model").clone();
    assert!(train::adapters_dir(&state.paths).join(&cm.id).join("adapter_model.safetensors").is_file());
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees";
    let t1 = std::time::Instant::now();
    let song = media::run(
        &state,
        store::new_id(),
        Request { op: "music".into(), prompt: "a song about a fresh start".into(), lyrics: Some(lyrics.into()), seconds: Some(40.0), seed: Some(3), language: Some("en".into()), music: Some(MusicSpec { custom: Some(cm.id.clone()), ..Default::default() }), ..Default::default() },
    )
    .await
    .unwrap()
    .remove(0);
    let file = std::path::PathBuf::from(r"C:\dev\tools\music\ab\custom.mp3");
    std::fs::write(&file, store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
    let wanted = sung_words(lyrics);
    println!("song with it in {:.0} s on {:?}: {} of {} words heard", t1.elapsed().as_secs_f64(), song.model_id, wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count(), wanted.len());
    assert_eq!(song.model_id.as_deref(), Some(train::BASE_MODEL));
    train::remove(&state.db.lock().unwrap(), &c, &state.paths, &cm.id).unwrap();
}

/// HeartMuLa installed from the catalog (audio.cpp's Vulkan build from its
/// release, the model checked against its hash) and a song made the
/// Studio's way: its length, the words Whisper hears, and the time. Kept in
/// C:\dev\tools\music\ab\heartmula\app.mp3.
///     cargo test e2e_heartmula -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_heartmula() {
    use crate::media::{self, store, MusicSpec, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let t = std::time::Instant::now();
    media::install_for_test(&state, "heartmula-3b").await.unwrap();
    println!("installed in {:.0} s", t.elapsed().as_secs_f64());
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let wanted = sung_words(lyrics);
    let t = std::time::Instant::now();
    let req = Request {
        op: "music".into(),
        model_id: Some("heartmula-3b".into()),
        prompt: "warm modern country about a fresh start".into(),
        lyrics: Some(lyrics.into()),
        seconds: Some(120.0),
        seed: Some(7),
        language: Some("en".into()),
        music: Some(MusicSpec { voice: Some("male".into()), instruments: vec!["acoustic guitar".into(), "pedal steel".into()], ..Default::default() }),
        ..Default::default()
    };
    let song = media::run(&state, store::new_id(), req).await.unwrap().remove(0);
    let file = std::path::PathBuf::from(r"C:\dev\tools\music\ab\heartmula\app.mp3");
    std::fs::write(&file, store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
    let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
    let matched = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
    println!("{:.0} s long in {:.0} s, {matched} of {} words heard, mime {}, title {:?}", song.seconds, t.elapsed().as_secs_f64(), wanted.len(), song.mime, song.title);
    assert_eq!(song.mime, "audio/mpeg");
    assert!(song.seconds > 30.0 && matched * 3 >= wanted.len() * 2);
}

/// Remaster and mashup as ACE-Step covers of the synth-pop song from
/// e2e_suno_features: the song re-rendered close to itself (strength 0.6,
/// 0.8), and its tune and words in the dark-country song's sound (that song
/// as the reference). The words Whisper hears in each, against the
/// original's. Songs in C:\dev\tools\music\ab\cover.
///     cargo test e2e_remaster_and_mashup -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_remaster_and_mashup() {
    use crate::media::music;
    let state = media_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab\cover");
    std::fs::create_dir_all(&out).unwrap();
    let source = std::path::PathBuf::from(r"C:\dev\tools\music\ab\features\song.mp3");
    let other = std::path::PathBuf::from(r"C:\dev\tools\music\ab\smoke-ace-step-1.5-xl.mp3");
    let lyrics = "[Verse 1]\nWindows down on the coast road\nRadio loud and the night is young\n\n[Chorus]\nSummer nights, we were golden\nDancing under the neon sun\nSummer nights, we were golden\nNever wanted the night to be done";
    let wanted = sung_words(lyrics);
    let words_in = |file: &std::path::Path| {
        let (heard, _) = heard_words(&whisper_hears(&state, file, Some("en")));
        wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count()
    };
    println!("original: {} of {} words heard", words_in(&source), wanted.len());
    let models = state.paths.models.join("media").join("music");
    let dir = crate::media::music_engine_dir(&state).expect("the music engine is installed");
    let runs: [(&str, &str, f32, bool); 4] = [
        ("remaster-0.6", "upbeat 80s-inspired synth pop about a summer drive, female lead vocals, polished modern production", 0.6, false),
        ("remaster-0.8", "upbeat 80s-inspired synth pop about a summer drive, female lead vocals, polished modern production", 0.8, false),
        ("mashup-0.5", "dark modern country, Southern gothic, acoustic guitar, pedal steel, female lead vocals", 0.5, true),
        ("mashup-0.7", "dark modern country, Southern gothic, acoustic guitar, pedal steel, female lead vocals", 0.7, true),
    ];
    for (name, caption, strength, with_other) in runs {
        let work = std::env::temp_dir().join(format!("sulcusai-cover-{}", crate::media::store::new_id()));
        std::fs::create_dir_all(&work).unwrap();
        let engine = music::Engine { dir: &dir, models: &models, low_priority: false, job: None, logs: &state.paths.logs };
        let r = music::SongRequest {
            caption,
            lyrics,
            seconds: 45.0,
            seed: 7,
            language: Some("en"),
            dit_file: "acestep-v15-xl-turbo-Q8_0.gguf",
            task: "cover",
            strength: Some(strength),
            source: Some(&source),
            reference: with_other.then_some(other.as_path()),
            versions: 1,
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let song = music::make(&engine, &work, &r, &AtomicBool::new(false), &|_, _| {}).await.unwrap().remove(0);
        let _ = std::fs::remove_dir_all(&work);
        let file = out.join(format!("{name}.mp3"));
        std::fs::write(&file, &song.mp3).unwrap();
        println!("{name}: {:.0} s, {:.0} s long, {} of {} words heard", t.elapsed().as_secs_f64(), song.seconds, words_in(&file), wanted.len());
    }
}

/// Cover art for a few kinds of song, kept in C:\dev\tools\music\ab\art to
/// look at (no lettering should be in them), and a song video of the first.
///     cargo test e2e_cover_art -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_cover_art() {
    use crate::media::{self, store, MediaItem, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    media::install_for_test(&state, "z-image-turbo").await.unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\music\ab\art");
    std::fs::create_dir_all(&out).unwrap();
    let song_mp3 = std::fs::read(r"C:\dev\tools\music\ab\features\song.mp3").expect("run e2e_suno_features first");
    let lyrics = "[Verse 1]\nWindows down on the coast road\nRadio loud and the night is young\n\n[Chorus]\nSummer nights, we were golden\nDancing under the neon sun\nSummer nights, we were golden\nNever wanted the night to be done";
    for (i, prompt) in ["upbeat 80s-inspired synth pop about a summer drive", "dark modern country, Southern gothic, a house built on ashes", "lo-fi hip hop for studying on a rainy night"].iter().enumerate() {
        let song = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "audio".into(), op: "music".into(), prompt: prompt.to_string(), title: Some("Summer Nights".into()), mime: "audio/mpeg".into(), seconds: 45.0, lyrics: Some(lyrics.into()), ..Default::default() }, &song_mp3, None).unwrap();
        let t = std::time::Instant::now();
        media::run(&state, store::new_id(), Request { op: "cover_art".into(), source: Some(song.id.clone()), seed: Some(3), ..Default::default() }).await.unwrap();
        std::fs::write(out.join(format!("art-{i}.jpg")), store::read_thumb(&state.paths, &c, &song.id).unwrap()).unwrap();
        println!("{prompt}: {:.0} s", t.elapsed().as_secs_f64());
        if i == 0 {
            let t = std::time::Instant::now();
            let v = media::run(&state, store::new_id(), Request { op: "song_video".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
            std::fs::write(out.join("video.mp4"), store::read(&state.paths, &c, &v.id).unwrap()).unwrap();
            std::fs::write(out.join("video-still.jpg"), store::read_thumb(&state.paths, &c, &v.id).unwrap()).unwrap();
            println!("video in {:.0} s: {} x {}, {:.0} s, {} bytes", t.elapsed().as_secs_f64(), v.width, v.height, v.seconds, v.size);
            assert_eq!(v.parent.as_deref(), Some(song.id.as_str()));
        }
    }
}

/// Compose with the real models. SULCUSAI_COMPOSE=plan (the default): the
/// chat model (SULCUSAI_COMPOSE_MODEL, default qwen3.5-9b) plans songs in
/// 4/4, 6/8 and 7/8, the arranger writes their notes, and each is kept in
/// SULCUSAI_MEDIA_OUT (default temp sulcusai-compose) as JSON and MIDI.
/// SULCUSAI_COMPOSE=arrange: those plans arranged again. =real: each one's
/// demo (`<name>.wav` beside it, rendered from its MIDI on the sound set as
/// the window does) is made real by the music model at SULCUSAI_STRENGTHS
/// (default 0.4,0.6,0.8). =listen: the sung ones made real, listened to.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_compose() {
    use crate::media::{self, compose, store};
    let state = media_state();
    let c = state.cipher().unwrap();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-compose").display().to_string()));
    std::fs::create_dir_all(&out).unwrap();
    let ask = |description: &str, beats, unit, era: &str, vocals, voice: &str, seed| compose::Ask {
        description: description.into(),
        beats,
        unit,
        sections: 6,
        vocals,
        voice: voice.into(),
        era: era.into(),
        seed,
        ..Default::default()
    };
    let asks = [
        ("pop-80s", ask("an upbeat synth-pop song about driving through the city at night", 4, 4, "1980s", true, "female", 11)),
        ("folk-6-8", ask("a gentle folk ballad about leaving the town you grew up in", 6, 8, "", true, "male", 12)),
        ("rock-7-8", ask("a driving instrumental rock piece with a big riff", 7, 8, "1970s", false, "", 13)),
        // From the Music tab: their own lyrics and song options.
        (
            "own-words",
            compose::Ask {
                song: Some(media::MusicSpec {
                    genre: Some("Country, Folk".into()),
                    emotion: Some("Heartbroken, Hopeful".into()),
                    vocal_style: Some("raspy".into()),
                    harmony_voice: Some("female".into()),
                    harmony_style: Some("thirds".into()),
                    harmony_where: Some("chorus".into()),
                    ..Default::default()
                }),
                lyrics: Some(
                    "[Verse]\nThe porch light's on but nobody's home\nI pack the truck and drive alone\nThe fields go gold behind the rain\nI won't come back this way again\n\n\
                     [Chorus]\nSo long to the town that raised me\nSo long to the river and the trees\nI'll carry you in every song\nSo long, so long\n\n[Instrumental]\n\n\
                     [Chorus]\nSo long to the town that raised me\nSo long to the river and the trees\nI'll carry you in every song\nSo long, so long"
                        .into(),
                ),
                ..ask("a heartbroken but hopeful country song about leaving home", 4, 4, "", true, "male", 14)
            },
        ),
    ];
    // A famous riff that's free to use (Grieg's "In the Hall of the Mountain
    // King", 1875), as 1970s hard rock: the plan by hand, the band by the
    // arranger, the theme as written (in E minor), an octave up in the chorus.
    let stage = std::env::var("SULCUSAI_COMPOSE").unwrap_or_else(|_| "plan".into());
    if stage == "riff" {
        let section = |name: &str, bars, drums: &str, bass: &str, keys: &str, energy| compose::PlanSection {
            name: name.into(),
            bars,
            chords: ["E5", "B5", "E5", "G5"].map(String::from).to_vec(),
            drums: drums.into(),
            bass: bass.into(),
            keys: keys.into(),
            melody: false,
            lines: Vec::new(),
            energy,
            take: 0,
        };
        let plan = compose::tidy(compose::Plan {
            title: "Mountain King".into(),
            bpm: 132,
            key: "E minor".into(),
            style: "hard rock".into(),
            era: "1970s".into(),
            description: "a driving hard rock instrumental, a big riff on distorted guitars".into(),
            instruments: compose::Instruments { chords: "distorted guitar".into(), melody: "distorted guitar".into(), bass: "electric bass".into(), drums: "rock kit".into() },
            sections: vec![
                section("Intro", 4, "light", "roots", "none", 2),
                section("Verse", 8, "steady", "pulse", "stabs", 3),
                section("Chorus", 8, "busy", "pulse", "held", 5),
                section("Outro", 4, "busy", "roots", "held", 4),
            ],
            seed: 5,
            ..Default::default()
        });
        let mut parts = compose::arrange(&plan);
        // The theme's four bars: (beat, length, pitch), an eighth note a step.
        let theme: [(f32, f32, u8); 26] = [
            (0.0, 0.5, 52), (0.5, 0.5, 54), (1.0, 0.5, 55), (1.5, 0.5, 57), (2.0, 0.5, 59), (2.5, 0.5, 55), (3.0, 1.0, 59),
            (4.0, 0.5, 58), (4.5, 0.5, 54), (5.0, 1.0, 58), (6.0, 0.5, 57), (6.5, 0.5, 53), (7.0, 1.0, 57),
            (8.0, 0.5, 52), (8.5, 0.5, 54), (9.0, 0.5, 55), (9.5, 0.5, 57), (10.0, 0.5, 59), (10.5, 0.5, 55), (11.0, 0.5, 59), (11.5, 0.5, 64),
            (12.0, 0.5, 62), (12.5, 0.5, 59), (13.0, 0.5, 55), (13.5, 0.5, 59), (14.0, 2.0, 62),
        ];
        let lead = parts.iter_mut().find(|p| p.name == "melody").unwrap();
        for (bar, up) in [(0, 0), (4, 0), (8, 0), (12, 12), (16, 12), (20, 0)] {
            lead.notes.extend(theme.iter().map(|(b, l, n)| compose::Note { beat: bar as f32 * 4.0 + b, len: l * 0.9, pitch: n + up, vel: 105 }));
        }
        let comp = compose::Composition { plan, parts };
        println!("mountain-king: {:.0}s, notes {}", comp.plan.seconds(), comp.parts.iter().map(|x| format!("{} {}", x.name, x.notes.len())).collect::<Vec<_>>().join(", "));
        std::fs::write(out.join("mountain-king.json"), serde_json::to_string_pretty(&comp).unwrap()).unwrap();
        std::fs::write(out.join("mountain-king.mid"), compose::midi(&comp)).unwrap();
        return;
    }
    // The plans written before, arranged again (after a change to the arranger).
    if stage == "arrange" {
        for (name, _) in &asks {
            let path = out.join(format!("{name}.json"));
            let old: compose::Composition = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let plan = compose::tidy(old.plan);
            let comp = compose::Composition { parts: compose::arrange(&plan), plan };
            println!("{name}: notes {}", comp.parts.iter().map(|x| format!("{} {}", x.name, x.notes.len())).collect::<Vec<_>>().join(", "));
            std::fs::write(&path, serde_json::to_string_pretty(&comp).unwrap()).unwrap();
            std::fs::write(out.join(format!("{name}.mid")), compose::midi(&comp)).unwrap();
        }
        return;
    }
    if stage == "plan" {
        let chat = std::env::var("SULCUSAI_COMPOSE_MODEL").unwrap_or_else(|_| "qwen3.5-9b".into());
        let spec = state.catalog.model(&chat).unwrap();
        let v = spec.variants.iter().find(|v| state.paths.models.join(&spec.id).join(&v.file).exists()).expect("the chat model is installed");
        {
            let conn = state.db.lock().unwrap();
            let path = state.paths.models.join(&spec.id).join(&v.file).display().to_string();
            crate::db::save_installed(&conn, &crate::db::InstalledModel { model_id: spec.id.clone(), quant: v.quant.clone(), path, size: v.size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
            crate::db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone())).unwrap();
        }
        for (name, a) in &asks {
            let t = std::time::Instant::now();
            let plan = compose::write_plan(&state, a).await.unwrap();
            let comp = compose::Composition { parts: compose::arrange(&plan), plan };
            let p = &comp.plan;
            println!(
                "{name} ({:.0}s): \"{}\" {} {} bpm {} era {:?}, {:.0}s; instruments {:?}",
                t.elapsed().as_secs_f64(),
                p.title,
                p.time(),
                p.bpm,
                p.key,
                p.era,
                p.seconds(),
                p.instruments
            );
            for s in &p.sections {
                println!("  {} {} bars [{}] drums {} bass {} keys {} energy {} lines {}", s.name, s.bars, s.chords.join(" "), s.drums, s.bass, s.keys, s.energy, s.lines.len());
            }
            println!("  notes: {}", comp.parts.iter().map(|x| format!("{} {}", x.name, x.notes.len())).collect::<Vec<_>>().join(", "));
            assert!(p.sections.len() >= 2 && p.seconds() > 20.0, "a whole song");
            assert_eq!((p.beats, p.unit), (a.beats, a.unit));
            assert_eq!(p.sections.iter().any(|s| !s.lines.is_empty()), a.vocals, "words only where asked");
            // Their own lyrics: their sections, in order, with their lines.
            if let Some(l) = &a.lyrics {
                let blocks = compose::lyric_blocks(l);
                assert_eq!(p.sections.iter().map(|s| (s.name.clone(), s.lines.clone())).collect::<Vec<_>>(), blocks);
                assert_eq!(p.song, a.song, "the options kept for Make it real");
            }
            std::fs::write(out.join(format!("{name}.json")), serde_json::to_string_pretty(&comp).unwrap()).unwrap();
            std::fs::write(out.join(format!("{name}.mid")), compose::midi(&comp)).unwrap();
        }
        state.engine.lock().await.stop().await;
        return;
    }
    // The versions made real, listened to: how many of the words come through.
    if stage == "listen" {
        let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
        crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
        for (name, a) in asks.iter().filter(|(_, a)| a.vocals) {
            let comp: compose::Composition = serde_json::from_slice(&std::fs::read(out.join(format!("{name}.json"))).unwrap()).unwrap();
            let mut files: Vec<_> = std::fs::read_dir(&out).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{name}-real-"))).collect();
            files.sort();
            for f in files {
                let mut song = media::add(&state, &c, std::fs::read(&f).unwrap(), "audio", "audio/mpeg", "import", &a.description, None, None, false).unwrap();
                song.seconds = comp.plan.seconds();
                song.lyrics = Some(comp.plan.lyrics());
                store::update(&state.db.lock().unwrap(), &c, &song).unwrap();
                let heard = media::run(&state, store::new_id(), media::Request { op: "listen".into(), source: Some(song.id.clone()), ..Default::default() }).await.unwrap().remove(0);
                let r = heard.review.unwrap();
                println!("{}: score {}, {} of {} words heard", f.file_name().unwrap().to_string_lossy(), r.score, r.heard, r.words);
            }
        }
        // The speech server the install started (left running, it outlived
        // the test and held its output open).
        crate::speech::stop(&state).await;
        return;
    }
    assert_eq!(stage, "real", "SULCUSAI_COMPOSE is plan, riff, arrange, real or listen");
    let model = std::env::var("SULCUSAI_FINISH_MODEL").ok().unwrap_or_else(|| media::recommended(&state, media::Kind::Music).unwrap().id);
    media::install_for_test(&state, &model).await.unwrap();
    let strengths = std::env::var("SULCUSAI_STRENGTHS").unwrap_or_else(|_| "0.4,0.6,0.8".into());
    for name in asks.iter().map(|a| a.0).chain(["mountain-king"]) {
        let Ok(wav) = std::fs::read(out.join(format!("{name}.wav"))) else { continue };
        let comp: compose::Composition = serde_json::from_slice(&std::fs::read(out.join(format!("{name}.json"))).unwrap()).unwrap();
        let mut demo = media::add(&state, &c, wav, "audio", "audio/wav", "compose", &comp.plan.description, None, None, false).unwrap();
        demo.seconds = comp.plan.seconds();
        demo.composition = Some(comp.clone());
        store::update(&state.db.lock().unwrap(), &c, &demo).unwrap();
        for s in strengths.split(',').filter_map(|s| s.trim().parse::<f32>().ok()) {
            let t = std::time::Instant::now();
            let mut req = media::real_request(&comp.plan, &demo.id, Some(s), Some(model.clone()), None, None, None, None);
            req.seed = Some(21);
            let real = media::run(&state, store::new_id(), req).await.unwrap().remove(0);
            println!("{name} at {s}: {:.0}s long, made in {:.0}s; caption words: {}", real.seconds, t.elapsed().as_secs_f64(), real.prompt);
            std::fs::write(out.join(format!("{name}-real-{s}.mp3")), store::read(&state.paths, &c, &real.id).unwrap()).unwrap();
        }
    }
}

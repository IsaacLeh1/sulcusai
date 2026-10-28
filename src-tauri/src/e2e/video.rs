// SPDX-License-Identifier: AGPL-3.0-only
//! Video: editing, dubbing, sound, captions, talking pictures.

use super::*;

/// Study material from a page of notes with a small model
/// (SULCUSAI_TEST_GGUF): flashcards, a quiz whose answers are among its
/// choices, a mind map; then an explainer video read by a natural voice,
/// whose sound speech recognition hears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_study_sets_and_an_explainer_video() {
    use crate::media::{self, explainer, podcast, store, Request};
    use crate::study::{self, Kind, MakeRequest};
    let state = media_state();
    let never = std::sync::atomic::AtomicBool::new(false);
    let notes = "Photosynthesis is how plants make their own food. In the chloroplasts, chlorophyll captures light energy. \
                 The plant takes in carbon dioxide through small pores in its leaves called stomata, and water through its roots. \
                 Using the light's energy it turns them into glucose, a sugar it uses for energy and growth, and releases oxygen as a by-product.\n\n\
                 The light-dependent reactions happen in the thylakoid membranes and make ATP and NADPH. \
                 The Calvin cycle happens in the stroma and uses that ATP and NADPH to fix carbon dioxide into sugar. \
                 Things that change the rate of photosynthesis include light intensity, carbon dioxide concentration and temperature.";
    let (eng, ep, name) = test_model().await;
    for kind in [Kind::Cards, Kind::Quiz, Kind::Map] {
        let started = std::time::Instant::now();
        let set = study::make_with(&state, &MakeRequest { kind: Some(kind), text: notes.into(), count: Some(6), language: Some("en".into()), ..Default::default() }, &ep, &name).await.unwrap();
        println!("{kind:?} in {:.0} s: {} cards, {} questions, map {:?}", started.elapsed().as_secs_f64(), set.cards.len(), set.quiz.len(), set.map.as_ref().map(|m| (&m.label, m.children.len())));
        match kind {
            Kind::Cards => {
                assert!(set.cards.len() >= 4);
                println!("  {} → {}", set.cards[0].front, set.cards[0].back);
            }
            Kind::Quiz => {
                assert!(set.quiz.len() >= 3);
                assert!(set.quiz.iter().all(|q| q.answer < q.choices.len()));
                println!("  {} [{}]", set.quiz[0].question, set.quiz[0].choices[set.quiz[0].answer]);
            }
            Kind::Map => assert!(set.map.as_ref().is_some_and(|m| m.children.len() >= 2)),
        }
    }
    let req = Request {
        op: "explainer".into(),
        prompt: notes.into(),
        language: Some("en".into()),
        talk: Some(podcast::Spec { speakers: vec![podcast::Speaker { name: "Narrator".into(), voice: Some("supertonic:F1".into()), about: None }], minutes: Some(1.0), ..Default::default() }),
        ..Default::default()
    };
    let deck = explainer::write_with(&state, &req, &ep, &name, &never, &|_, _| {}).await.unwrap();
    drop(eng);
    let parsed: explainer::Deck = serde_json::from_str(&deck).unwrap();
    println!("deck “{}”: {} slides", parsed.title, parsed.slides.len());
    assert!(parsed.slides.len() >= 3);
    crate::natural::install_for_test(&state).await.unwrap();
    let started = std::time::Instant::now();
    let item = media::run(&state, store::new_id(), Request { lyrics: Some(deck), ..req }).await.unwrap().remove(0);
    println!("video: {:.0} s, made in {:.0} s", item.seconds, started.elapsed().as_secs_f64());
    assert_eq!(item.mime, "video/mp4");
    assert!(item.seconds > 15.0);
    let path = std::path::PathBuf::from(r"C:\dev\tools\voice\tests\e2e-explainer.mp4");
    std::fs::write(&path, store::read(&state.paths, &state.work_cipher().unwrap(), &item.id).unwrap()).unwrap();
    let (pcm, rate, ch) = media::audio_in::decode(&path).unwrap();
    let mono: Vec<f32> = pcm.chunks(ch.max(1) as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).take(rate as usize * 30).collect();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    let heard = crate::speech::transcribe(&ep, &crate::audio::resample(&mono, rate, crate::audio::RATE), &crate::speech::Options { language: Some("en".into()), ..Default::default() }).await.unwrap();
    println!("the video's first 30 s: {}", heard.text);
    assert!(heard.text.to_lowercase().contains("photosynth") || heard.text.to_lowercase().contains("plant"), "{}", heard.text);
    crate::speech::stop(&state).await;
}

/// Videos edited with FFmpeg (downloaded the first time): a clip cut to a
/// stretch, joined with itself, given a song as its sound, the song mixed
/// in, and its sound taken off; each checked by what FFprobe finds in it.
///     cargo test e2e_videos_are_edited -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_videos_are_edited() {
    use crate::media::{self, store, MediaItem, Request, VideoEdit};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let clip = std::fs::read(r"C:\dev\tools\voice\tests\e2e-explainer.mp4").unwrap();
    let video = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "video".into(), mime: "video/mp4".into(), op: "import".into(), prompt: "an explainer".into(), seconds: 60.0, ..Default::default() }, &clip, None).unwrap();
    let song = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "audio".into(), mime: "audio/mpeg".into(), op: "import".into(), prompt: "a song".into(), seconds: 100.0, ..Default::default() }, &std::fs::read(r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3").unwrap(), None).unwrap();
    let edit = |e: VideoEdit| Request { op: "video_edit".into(), source: Some(video.id.clone()), edit: Some(e), ..Default::default() };
    let probe = |item: &MediaItem| {
        let p = std::env::temp_dir().join(format!("{}.mp4", item.id));
        std::fs::write(&p, store::read(&state.paths, &c, &item.id).unwrap()).unwrap();
        p
    };
    let cut = media::run(&state, store::new_id(), edit(VideoEdit { trim: Some([2.0, 7.0]), ..Default::default() })).await.unwrap().remove(0);
    let joined = media::run(&state, store::new_id(), edit(VideoEdit { trim: Some([0.0, 4.0]), join: vec![cut.id.clone()], ..Default::default() })).await.unwrap().remove(0);
    let sung = media::run(&state, store::new_id(), edit(VideoEdit { trim: Some([0.0, 6.0]), audio: Some(song.id.clone()), ..Default::default() })).await.unwrap().remove(0);
    let mixed = media::run(&state, store::new_id(), edit(VideoEdit { trim: Some([0.0, 6.0]), audio: Some(song.id.clone()), audio_mode: Some("mix".into()), volume: Some(0.5), ..Default::default() })).await.unwrap().remove(0);
    let quiet = media::run(&state, store::new_id(), edit(VideoEdit { trim: Some([0.0, 3.0]), mute: true, ..Default::default() })).await.unwrap().remove(0);
    let engines = Paths::new(app_data_dir()).unwrap().engines;
    let ffprobe = walkdir_find(&engines, "ffprobe.exe").expect("FFprobe was downloaded");
    for (name, item, secs, sound) in [("cut", &cut, 5.0, true), ("joined", &joined, 9.0, true), ("sung", &sung, 6.0, true), ("mixed", &mixed, 6.0, true), ("quiet", &quiet, 3.0, false)] {
        let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &probe(item)).await;
        println!("{name}: {:.2} s, {}x{}, sound: {}, still: {}", p.seconds, p.width, p.height, p.audio, item.poster);
        assert!((p.seconds - secs).abs() < 0.6, "{name} is {:.2} s, not {secs}", p.seconds);
        assert_eq!(p.audio, sound, "{name}'s sound");
        assert!(item.poster, "{name} has a still");
    }
    std::fs::write(r"C:\dev\tools\voice\tests\e2e-video-sung.mp4", store::read(&state.paths, &c, &sung.id).unwrap()).unwrap();
}

/// The explainer's first 20 s dubbed into Spanish: whisper hears it, Qwen3
/// 4B (installed) translates, a natural voice says it; whisper then hears
/// Spanish in the dubbed video, as long as the clip. Written to
/// C:\dev\tools\voice\tests\e2e-video-dubbed.mp4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_video_is_dubbed() {
    let clip = std::fs::read(r"C:\dev\tools\voice\tests\e2e-explainer.mp4").unwrap();
    dubbed_for_test(&clip, 60.0, 20.0, None, r"C:\dev\tools\voice\tests\e2e-video-dubbed.mp4").await;
}

/// Two people talking (the made podcast, as a video) dubbed into Spanish in
/// each one's own voice: the speaker model tells them apart and Qwen3-TTS
/// speaks each one's lines in a voice made from theirs. Written to
/// C:\dev\tools\voice\tests\e2e-video-dubbed-voices.mp4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_video_is_dubbed_in_its_speakers_voices() {
    let real = Paths::new(app_data_dir()).unwrap();
    let ffmpeg = walkdir_find(&real.engines, "ffmpeg.exe").expect("FFmpeg was downloaded");
    let video = std::env::temp_dir().join("e2e-podcast-video.mp4");
    let ok = std::process::Command::new(&ffmpeg)
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", "color=c=0x334455:s=640x360:r=25", "-i", r"C:\dev\tools\voice\tests\e2e-podcast.mp3", "-t", "30", "-shortest", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .arg(&video)
        .status()
        .unwrap();
    assert!(ok.success());
    let clip = std::fs::read(&video).unwrap();
    dubbed_for_test(&clip, 30.0, 30.0, Some("each"), r"C:\dev\tools\voice\tests\e2e-video-dubbed-voices.mp4").await;
}

/// `clip` (a video `secs` long) cut to its first `keep` s and dubbed into
/// Spanish with `voice`, as the app would, the chat model that translates
/// loaded first. Written to `out`; whisper must hear Spanish in it.
async fn dubbed_for_test(clip: &[u8], secs: f64, keep: f64, voice: Option<&str>, out: &str) {
    use crate::media::{self, store, MediaItem, Request, VideoEdit};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let never = std::sync::atomic::AtomicBool::new(false);
    crate::natural::install_for_test(&state).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    // The chat model that translates, loaded as the app would.
    let real = Paths::new(app_data_dir()).unwrap();
    let spec = state.catalog.model("qwen3-4b-2507").unwrap().clone();
    let budget = Budget::from_hardware(&state.hardware.read().unwrap());
    let exe = engine::installed_server(&real, &state.catalog.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let file = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let (ctx, layers) = catalog::launch_settings(&spec, "Q4_K_M", &budget);
    state.engine.lock().await.ensure(engine::LaunchSpec { exe: &exe, model_path: &file, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None).await.unwrap();

    let video = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "video".into(), mime: "video/mp4".into(), op: "import".into(), prompt: "a clip".into(), seconds: secs, ..Default::default() }, clip, None).unwrap();
    let part = media::run(&state, store::new_id(), Request { op: "video_edit".into(), source: Some(video.id.clone()), edit: Some(VideoEdit { trim: Some([0.0, keep]), ..Default::default() }), ..Default::default() }).await.unwrap().remove(0);
    let started = std::time::Instant::now();
    let dubbed = media::run(&state, store::new_id(), Request { op: "dub".into(), source: Some(part.id.clone()), language: Some("es".into()), voice: voice.map(Into::into), ..Default::default() }).await.unwrap().remove(0);
    println!("dubbed in {:.0} s", started.elapsed().as_secs_f64());
    state.engine.lock().await.stop().await;
    let out = std::path::PathBuf::from(out);
    std::fs::write(&out, store::read(&state.paths, &c, &dubbed.id).unwrap()).unwrap();
    let ffprobe = walkdir_find(&real.engines, "ffprobe.exe").expect("FFprobe was downloaded");
    let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &out).await;
    println!("dubbed: {:.2} s, {}x{}, sound: {}", p.seconds, p.width, p.height, p.audio);
    assert!((p.seconds - keep).abs() < 0.6 && p.audio && dubbed.poster);
    let ffmpeg = ffprobe.with_file_name("ffmpeg.exe");
    let wav = std::env::temp_dir().join(format!("dubbed-{}.wav", dubbed.id));
    let ok = std::process::Command::new(&ffmpeg).args(["-y", "-v", "error", "-i"]).arg(&out).args(["-ac", "1", "-ar", "16000"]).arg(&wav).status().unwrap();
    assert!(ok.success());
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    let (samples, _, _) = crate::media::audio_in::decode(&wav).unwrap();
    let heard = crate::speech::transcribe(&ep, &samples, &crate::speech::Options { language: Some("auto".into()), ..Default::default() }).await.unwrap();
    println!("heard ({:?}): {}", heard.language, heard.text);
    assert!(heard.language.as_deref().is_some_and(|l| l == "es" || l.eq_ignore_ascii_case("spanish")), "Spanish is heard");
}

/// Sound made for an 8 s clip by the music model (ACE-Step 1.5) and mixed
/// with its own: the result is as long as the clip, with a sound kept in
/// the gallery for it. Written to C:\dev\tools\voice\tests\e2e-video-sound.mp4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_video_gets_sound() {
    use crate::media::{self, store, MediaItem, Request, VideoEdit};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    media::install_for_test(&state, "ace-step-1.5").await.unwrap();
    let clip = std::fs::read(r"C:\dev\tools\voice\tests\e2e-explainer.mp4").unwrap();
    let video = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "video".into(), mime: "video/mp4".into(), op: "import".into(), prompt: "a leaf in the sun".into(), seconds: 60.0, ..Default::default() }, &clip, None).unwrap();
    let part = media::run(&state, store::new_id(), Request { op: "video_edit".into(), source: Some(video.id.clone()), edit: Some(VideoEdit { trim: Some([0.0, 8.0]), ..Default::default() }), ..Default::default() }).await.unwrap().remove(0);
    let started = std::time::Instant::now();
    let made = media::run(&state, store::new_id(), Request { op: "video_sound".into(), source: Some(part.id.clone()), prompt: "soft piano and birdsong".into(), ..Default::default() }).await.unwrap();
    println!("made in {:.0} s: {:?}", started.elapsed().as_secs_f64(), made.iter().map(|m| (&m.kind, &m.op)).collect::<Vec<_>>());
    let with = made.iter().find(|m| m.kind == "video").expect("a video");
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests\e2e-video-sound.mp4");
    std::fs::write(&out, store::read(&state.paths, &c, &with.id).unwrap()).unwrap();
    let ffprobe = walkdir_find(&Paths::new(app_data_dir()).unwrap().engines, "ffprobe.exe").expect("FFprobe was downloaded");
    let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &out).await;
    println!("with sound: {:.2} s, sound: {}", p.seconds, p.audio);
    assert!((p.seconds - 8.0).abs() < 0.6 && p.audio && with.op == "video_sound");
    let sounds = store::list(&state.db.lock().unwrap(), &c, Some("audio"), 10, None);
    assert!(!sounds.is_empty(), "the sound made is in the gallery too");
}

/// A clip with its sound made together (LTX-2.3, installed by the test,
/// about 26 GB): 5 s from a description with speech and a crowd in it.
/// It comes out as an MP4 with sound, as long as asked. Written to
/// C:\dev\tools\voice\tests\e2e-video-with-sound.mp4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_video_comes_with_its_sound() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    media::install_for_test(&state, "ltx-2.3-distilled").await.unwrap();
    let started = std::time::Instant::now();
    let prompt = "A man in a blue jacket stands at a microphone on a small outdoor stage and says \"Good evening, everybody!\" The crowd in front of him cheers and claps.";
    let made = media::run(&state, store::new_id(), Request { op: "video".into(), model_id: Some("ltx-2.3-distilled".into()), prompt: prompt.into(), seconds: Some(5.0), ..Default::default() }).await.unwrap().remove(0);
    println!("made in {:.0} s", started.elapsed().as_secs_f64());
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests\e2e-video-with-sound.mp4");
    std::fs::write(&out, store::read(&state.paths, &c, &made.id).unwrap()).unwrap();
    let ffprobe = walkdir_find(&Paths::new(app_data_dir()).unwrap().engines, "ffprobe.exe").expect("FFprobe was downloaded");
    let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &out).await;
    println!("with sound: {:.2} s, {}x{} at {:.0} fps, sound: {}", p.seconds, p.width, p.height, p.fps, p.audio);
    assert!(p.audio && made.mime == "video/mp4", "the clip has its sound");
    assert!((p.seconds - 5.0).abs() < 0.4, "{} s", p.seconds);
}

/// A portrait made to talk (Wan 2.2 S2V, installed by the test, about
/// 19 GB, with its 4-step LoRA on its own engine build): words read aloud
/// by a natural voice, about 6 s, so two parts, at the model's own settings.
/// The video is as long as the voice, with its sound. Written to
/// C:\dev\tools\voice\tests\e2e-talking.mp4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_picture_talks() {
    use crate::media::{self, store, MediaItem, Request};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    crate::natural::install_for_test(&state).await.unwrap();
    media::install_for_test(&state, "wan2.2-s2v-14b").await.unwrap();
    let png = std::fs::read(r"C:\dev\tools\lora-test\hero.png").unwrap();
    let face = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "image".into(), mime: "image/png".into(), op: "import".into(), prompt: "a woman in a yellow raincoat".into(), width: 1024, height: 1024, ..Default::default() }, &png, None).unwrap();
    let words = "Hello there! This is a picture that talks, and my lips move with my words.";
    let started = std::time::Instant::now();
    let made = media::run(&state, store::new_id(), Request { op: "speak".into(), source: Some(face.id.clone()), say: Some(words.into()), language: Some("en".into()), seconds: Some(6.0), ..Default::default() }).await.unwrap().remove(0);
    println!("talked in {:.0} s", started.elapsed().as_secs_f64());
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests\e2e-talking.mp4");
    std::fs::write(&out, store::read(&state.paths, &c, &made.id).unwrap()).unwrap();
    let ffprobe = walkdir_find(&Paths::new(app_data_dir()).unwrap().engines, "ffprobe.exe").expect("FFprobe was downloaded");
    let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &out).await;
    println!("talking: {:.2} s, {}x{} at {:.0} fps, sound: {}", p.seconds, p.width, p.height, p.fps, p.audio);
    assert!(p.audio && made.poster && made.op == "speak");
    assert!((p.seconds - made.seconds).abs() < 0.3 && made.seconds > 5.0, "{} s for {} s of voice", p.seconds, made.seconds);
}

/// A portrait made to talk the quick way (Ditto, set up by the test the
/// first time: its packages through uv, its model files from Hugging Face):
/// words read aloud by a natural voice, then a picture with no face, which
/// says so. Until its package is on the `engines` release,
/// SULCUSAI_DITTO_ZIP points at the zip `scripts/ditto/package.py` made.
/// Written to C:\dev\tools\voice\tests\e2e-talking-quick.mp4.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_picture_talks_quickly() {
    use crate::media::{self, store, MediaItem, Request};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    crate::natural::install_for_test(&state).await.unwrap();
    if let (Some(zip), Some(d)) = (std::env::var_os("SULCUSAI_DITTO_ZIP"), state.catalog.media.ditto.as_ref()) {
        let dir = media::ditto::source_dir(&state.paths, d);
        if !dir.join("talk.py").is_file() {
            crate::engine::unzip_for_test(std::path::Path::new(&zip), &dir).unwrap();
        }
    }
    let png = std::fs::read(r"C:\dev\tools\lora-test\hero.png").unwrap();
    let face = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "image".into(), mime: "image/png".into(), op: "import".into(), prompt: "a woman in a yellow raincoat".into(), width: 1024, height: 1024, ..Default::default() }, &png, None).unwrap();
    let words = "Hello there! This is a picture that talks the quick way, and my lips move with my words.";
    let started = std::time::Instant::now();
    let made = media::run(&state, store::new_id(), Request { op: "talk".into(), source: Some(face.id.clone()), say: Some(words.into()), language: Some("en".into()), seconds: Some(6.0), ..Default::default() }).await.unwrap().remove(0);
    println!("talked in {:.0} s", started.elapsed().as_secs_f64());
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests\e2e-talking-quick.mp4");
    std::fs::write(&out, store::read(&state.paths, &c, &made.id).unwrap()).unwrap();
    let ffprobe = walkdir_find(&Paths::new(app_data_dir()).unwrap().engines, "ffprobe.exe").expect("FFprobe was downloaded");
    let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &out).await;
    println!("talking: {:.2} s, {}x{} at {:.0} fps, sound: {}", p.seconds, p.width, p.height, p.fps, p.audio);
    assert!(p.audio && made.poster && made.op == "speak" && made.model_id.as_deref() == Some("ditto"));
    assert_eq!((p.width, p.height), (1024, 1024), "the picture keeps its size");
    assert!((p.fps - 25.0).abs() < 0.5);
    assert!((p.seconds - made.seconds).abs() < 0.3 && made.seconds > 4.0, "{} s for {} s of voice", p.seconds, made.seconds);

    // A picture with no face in it.
    let sky = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(512, 512, |_, y| image::Rgb([90, 140, 200u8.saturating_sub((y / 8) as u8)])));
    let mut bytes = Vec::new();
    sky.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    let blank = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "image".into(), mime: "image/png".into(), op: "import".into(), prompt: "sky".into(), width: 512, height: 512, ..Default::default() }, &bytes, None).unwrap();
    let e = media::run(&state, store::new_id(), Request { op: "talk".into(), source: Some(blank.id.clone()), say: Some("Hello.".into()), language: Some("en".into()), ..Default::default() }).await.unwrap_err();
    println!("no face: {e}");
    assert!(e.contains("No face"), "{e}");
}

/// The quick way on a PC without an NVIDIA card (this one's card is made to
/// look like an Intel one): Ditto runs on the processor from its own
/// packages, and the picture still talks. SULCUSAI_DITTO_ZIP as above.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_picture_talks_on_the_processor() {
    use crate::media::{self, store, MediaItem, Request};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    for g in state.hardware.write().unwrap().gpus.iter_mut() {
        g.vendor = "Intel".into();
    }
    assert!(!crate::natural::nvidia_here(&state));
    crate::natural::install_for_test(&state).await.unwrap();
    let d = state.catalog.media.ditto.clone().unwrap();
    if let Some(zip) = std::env::var_os("SULCUSAI_DITTO_ZIP") {
        let dir = media::ditto::source_dir(&state.paths, &d);
        if !dir.join("talk.py").is_file() {
            crate::engine::unzip_for_test(std::path::Path::new(&zip), &dir).unwrap();
        }
    }
    let png = std::fs::read(r"C:\dev\tools\lora-test\hero.png").unwrap();
    let face = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "image".into(), mime: "image/png".into(), op: "import".into(), prompt: "a woman in a yellow raincoat".into(), width: 1024, height: 1024, ..Default::default() }, &png, None).unwrap();
    let started = std::time::Instant::now();
    let made = media::run(&state, store::new_id(), Request { op: "talk".into(), source: Some(face.id.clone()), say: Some("Hello there, this picture talks on the processor.".into()), language: Some("en".into()), seconds: Some(4.0), ..Default::default() }).await.unwrap().remove(0);
    println!("talked on the processor in {:.0} s ({:.1} s of voice)", started.elapsed().as_secs_f64(), made.seconds);
    assert!(media::ditto::ready(&state.paths, &d, true), "the processor's packages are set up");
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests\e2e-talking-processor.mp4");
    std::fs::write(&out, store::read(&state.paths, &c, &made.id).unwrap()).unwrap();
    let ffprobe = walkdir_find(&Paths::new(app_data_dir()).unwrap().engines, "ffprobe.exe").expect("FFprobe was downloaded");
    let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &out).await;
    println!("talking: {:.2} s, {}x{} at {:.0} fps, sound: {}", p.seconds, p.width, p.height, p.fps, p.audio);
    assert!(p.audio && made.model_id.as_deref() == Some("ditto"));
    assert_eq!((p.width, p.height), (1024, 1024));
    assert!((p.seconds - made.seconds).abs() < 0.3 && made.seconds > 2.0);
    let log = std::fs::read_to_string(state.paths.logs.join("talking-picture.log")).unwrap_or_default();
    assert!(log.contains("--processor") || log.contains("SULCUS"), "it ran");
}

/// Captions on the explainer's first 20 s, translated into Spanish by Qwen3
/// 4B (installed): a new video with them drawn on, its sound kept, and the
/// captions' text with it. A frame goes to
/// C:\dev\tools\voice\tests\e2e-captions.jpg.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_video_gets_captions() {
    use crate::media::{self, store, MediaItem, Request, VideoEdit};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let never = std::sync::atomic::AtomicBool::new(false);
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    let real = Paths::new(app_data_dir()).unwrap();
    let spec = state.catalog.model("qwen3-4b-2507").unwrap().clone();
    let budget = Budget::from_hardware(&state.hardware.read().unwrap());
    let exe = engine::installed_server(&real, &state.catalog.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let file = real.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let (ctx, layers) = catalog::launch_settings(&spec, "Q4_K_M", &budget);
    state.engine.lock().await.ensure(engine::LaunchSpec { exe: &exe, model_path: &file, model_id: &spec.id, quant: "Q4_K_M", ctx, gpu_layers: layers, threads: 0, low_priority: false, log: &real.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None).await.unwrap();
    let clip = std::fs::read(r"C:\dev\tools\voice\tests\e2e-explainer.mp4").unwrap();
    let video = store::save(&state.db.lock().unwrap(), &state.paths, &c, MediaItem { kind: "video".into(), mime: "video/mp4".into(), op: "import".into(), prompt: "an explainer".into(), seconds: 60.0, ..Default::default() }, &clip, None).unwrap();
    let part = media::run(&state, store::new_id(), Request { op: "video_edit".into(), source: Some(video.id.clone()), edit: Some(VideoEdit { trim: Some([0.0, 20.0]), ..Default::default() }), ..Default::default() }).await.unwrap().remove(0);
    let made = media::run(&state, store::new_id(), Request { op: "captions".into(), source: Some(part.id.clone()), language: Some("es".into()), ..Default::default() }).await.unwrap().remove(0);
    state.engine.lock().await.stop().await;
    let text = made.lyrics.clone().unwrap_or_default();
    println!("{}", text.lines().take(8).collect::<Vec<_>>().join("\n"));
    assert!(made.op == "captions" && text.contains("-->") && made.poster);
    let out = std::env::temp_dir().join(format!("{}.mp4", made.id));
    std::fs::write(&out, store::read(&state.paths, &c, &made.id).unwrap()).unwrap();
    let ffprobe = walkdir_find(&real.engines, "ffprobe.exe").unwrap();
    let p = crate::media::ffmpeg_probe_for_test(&ffprobe, &out).await;
    assert!((p.seconds - 20.0).abs() < 0.6 && p.audio);
    let shot = std::process::Command::new(ffprobe.with_file_name("ffmpeg.exe")).args(["-y", "-v", "error", "-ss", "4", "-i"]).arg(&out).args(["-frames:v", "1"]).arg(r"C:\dev\tools\voice\tests\e2e-captions.jpg").status().unwrap();
    assert!(shot.success());
}

fn walkdir_find(root: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = walkdir_find(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)) {
            return Some(p);
        }
    }
    None
}

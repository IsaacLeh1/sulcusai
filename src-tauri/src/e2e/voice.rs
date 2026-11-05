// SPDX-License-Identifier: AGPL-3.0-only
//! Speech, dictation, meetings and voices.

use super::*;

/// Your voice through the jobs as the app runs them: a voice from
/// qwentts.cpp's example speech (split at its pauses into three read
/// lines), narration in it, and a gallery song sung again in it. The engine
/// and models download from the catalog the first time, into the real
/// engines and models folders, as the installed app would.
/// SULCUSAI_SONG picks the song (default a Compose test song).
///     cargo test e2e_own_voice_reads_and_sings -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_own_voice_reads_and_sings() {
    use crate::media::{self, own_voice, store, Request};
    let state = media_state();
    let cipher = state.work_cipher().unwrap();
    let example = std::path::PathBuf::from(r"C:\dev\tools\voice\qwentts.cpp\examples");
    let pcm = crate::audio::wav_decode(&std::fs::read(example.join("freeman.wav")).unwrap()).unwrap();
    let speech = media::stems::resample(&pcm.samples, pcm.rate, 48_000);
    // The two longest pauses inside it, as the line breaks.
    let frame = 960;
    let rms: Vec<f32> = speech.chunks(frame).map(crate::audio::rms).collect();
    let quiet = rms.iter().copied().fold(f32::MAX, f32::min).max(1e-4) * 4.0;
    let mut pauses: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, r) in rms.iter().enumerate() {
        match (start, *r < quiet) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                if s > 10 && i < rms.len() - 10 {
                    pauses.push((i - s, (s + i) / 2));
                }
                start = None;
            }
            _ => {}
        }
    }
    pauses.sort_by(|a, b| b.0.cmp(&a.0));
    let mut cuts: Vec<usize> = pauses.iter().take(2).map(|p| p.1 * frame).collect();
    cuts.sort();
    assert_eq!(cuts.len(), 2, "two pauses in the example");
    let words = std::fs::read_to_string(example.join("freeman.txt")).unwrap();
    let sentences: Vec<&str> = words.trim().split_inclusive(". ").collect();
    let third = |i: usize| sentences.iter().skip(i * sentences.len() / 3).take(sentences.len() / 3 + usize::from(i == 2)).copied().collect::<String>();
    let voice = own_voice::add_for_test(
        &state,
        "Test voice",
        &[
            ("everyday", speech[..cuts[0]].to_vec(), &third(0)),
            ("intro", speech[cuts[0]..cuts[1]].to_vec(), &third(1)),
            ("story", speech[cuts[1]..].to_vec(), &third(2)),
        ],
    );
    let started = std::time::Instant::now();
    let read = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: "The bakery opens at seven. Bring the blue umbrella, it might rain later.".into(), voice: Some(format!("own:{voice}")), language: Some("en".into()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    println!("narration: {:.1} s of speech in {:.0} s (first time includes the downloads)", read.seconds, started.elapsed().as_secs_f64());
    assert!(read.seconds > 2.0);
    // A song in the gallery, sung again in the voice.
    let song_path = std::env::var("SULCUSAI_SONG").unwrap_or_else(|_| r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3".into());
    let bytes = std::fs::read(&song_path).unwrap();
    let item = media::MediaItem { kind: "audio".into(), mime: "audio/mpeg".into(), op: "import".into(), prompt: "a song".into(), seconds: 100.7, ..Default::default() };
    let song = store::save(&state.db.lock().unwrap(), &state.paths, &cipher, item, &bytes, None).unwrap();
    let started = std::time::Instant::now();
    let sung = media::run(&state, store::new_id(), Request { op: "revoice".into(), prompt: String::new(), source: Some(song.id.clone()), voice: Some(voice.clone()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    println!("sung again: {:.0} s song in {:.0} s; note: {:?}", sung.seconds, started.elapsed().as_secs_f64(), sung.note);
    assert_eq!(sung.parent.as_deref(), Some(song.id.as_str()));
    assert!((sung.seconds - song.seconds).abs() < 2.0, "{} vs {}", sung.seconds, song.seconds);
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests");
    std::fs::write(out.join("e2e-narration.wav"), store::read(&state.paths, &cipher, &read.id).unwrap()).unwrap();
    std::fs::write(out.join("e2e-sung.wav"), store::read(&state.paths, &cipher, &sung.id).unwrap()).unwrap();
}

/// A voice made from words (VoiceDesign, downloaded the first time), then a
/// short podcast written by a small model (SULCUSAI_TEST_GGUF) and read by
/// two people: the made voice and a natural voice picked for the other.
/// Whisper hears the made voice's sample and the podcast's opening; the
/// files go to C:\dev\tools\voice\tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_voice_from_words_and_a_podcast() {
    use crate::media::{self, own_voice, podcast, store, Request};
    let state = media_state();
    let cipher = state.work_cipher().unwrap();
    let never = std::sync::atomic::AtomicBool::new(false);
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\tests");
    std::fs::create_dir_all(&out).unwrap();
    crate::natural::install_for_test(&state).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();

    let started = std::time::Instant::now();
    let description = "A warm older woman with a low, calm voice, speaking slowly and kindly";
    let made = own_voice::design(&state, description, Some("en"), &never, &|s, f| println!("  {s} {:.0}%", f * 100.0)).await.unwrap();
    println!("made {} voices in {:.0} s: {:?} s long", made.len(), started.elapsed().as_secs_f64(), made.iter().map(|m| (m.len() as f32 / 24_000.0 * 10.0).round() / 10.0).collect::<Vec<_>>());
    assert!(made.len() >= 2, "three were asked for");
    for (i, m) in made.iter().enumerate() {
        std::fs::write(out.join(format!("e2e-made-{}.wav", i + 1)), crate::audio::wav_encode(m, 24_000)).unwrap();
    }
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    let heard = crate::speech::transcribe(&ep, &crate::audio::resample(&made[0], 24_000, crate::audio::RATE), &crate::speech::Options { language: Some("en".into()), ..Default::default() }).await.unwrap();
    println!("the made voice said: {}", heard.text);
    assert!(heard.text.to_lowercase().contains("sea"), "{}", heard.text);
    let voice = own_voice::keep_for_test(&state, "Rose", description, &made[0]);

    // The script, from the test model; then the card is let go for the voices.
    let (eng, llm, name) = test_model().await;
    let talk = podcast::Spec {
        speakers: vec![
            podcast::Speaker { name: "Rose".into(), voice: Some(format!("own:{voice}")), about: Some("the host".into()) },
            podcast::Speaker { name: "Ben".into(), voice: None, about: Some("a baker who loves sourdough".into()) },
        ],
        minutes: Some(1.5),
        style: Some("interview".into()),
        ..Default::default()
    };
    let req = Request { op: "podcast".into(), prompt: "How sourdough bread rises, and how to keep a starter alive".into(), language: Some("en".into()), talk: Some(talk), ..Default::default() };
    let started = std::time::Instant::now();
    let script = podcast::write_with(&state, &req, &llm, &name, &never, &|s, _| println!("  {s}")).await.unwrap();
    println!("written by {name} in {:.0} s:\n{script}\n", started.elapsed().as_secs_f64());
    drop(eng);
    let (_, turns) = podcast::parse(&script, &req.talk.as_ref().unwrap().speakers);
    assert!(turns.len() >= 6, "a conversation: {} turns", turns.len());
    assert!(turns.iter().any(|t| t.speaker == 0) && turns.iter().any(|t| t.speaker == 1), "both talk");

    let started = std::time::Instant::now();
    let item = media::run(&state, store::new_id(), Request { lyrics: Some(script), ..req }).await.unwrap().remove(0);
    println!("podcast “{}”: {:.0} s, read in {:.0} s", item.title.clone().unwrap_or_default(), item.seconds, started.elapsed().as_secs_f64());
    assert!(item.seconds > 30.0, "{}", item.seconds);
    assert!(item.lyrics.as_deref().is_some_and(|l| l.contains("Rose:") && l.contains("Ben:")));
    let path = out.join(if item.mime == "audio/mpeg" { "e2e-podcast.mp3" } else { "e2e-podcast.wav" });
    std::fs::write(&path, store::read(&state.paths, &cipher, &item.id).unwrap()).unwrap();
    let (pcm, rate, channels) = media::audio_in::decode(&path).unwrap();
    let mono: Vec<f32> = pcm.chunks(channels.max(1) as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).take(rate as usize * 40).collect();
    let heard = crate::speech::transcribe(&ep, &crate::audio::resample(&mono, rate, crate::audio::RATE), &crate::speech::Options { language: Some("en".into()), ..Default::default() }).await.unwrap();
    println!("the podcast's first 40 s: {}", heard.text);
    let low = heard.text.to_lowercase();
    assert!(["sourdough", "bread", "starter", "welcome"].iter().any(|w| low.contains(w)), "{}", heard.text);
    crate::speech::stop(&state).await;
}

/// A voice from a podcast episode alone: intro music, a host who talks
/// most and a guest (two natural voices), with music under the host's first
/// lines. The voice made from it keeps the host, hears the words, and reads
/// new words sounding like the host, not the guest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_voice_from_a_podcast_recording() {
    use crate::media::{self, own_voice, stems, store, Request};
    let state = media_state();
    let never = std::sync::atomic::AtomicBool::new(false);
    crate::natural::install_for_test(&state).await.unwrap();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    const RATE: u32 = 44_100;
    let say = |text: &str, voice: &str| {
        let s = crate::tts::synthesize(text, Some(voice), Some("en"), 1.0).unwrap();
        stems::resample(&s.samples.iter().map(|x| *x as f32 / 32768.0).collect::<Vec<_>>(), s.rate, RATE)
    };
    let (host, guest) = ("supertonic:M2", "supertonic:F3");
    let host_lines = [
        "Welcome back to the show, everybody. Today we are talking about how small towns can keep their main streets alive.",
        "I grew up in a place where every shop closed by six, and on Sundays you could hear a pin drop on the sidewalk.",
        "So the first question I always ask is simple. Who actually walks down main street on a normal Tuesday morning?",
        "That is a great point, and it connects to something the mayor told me last spring about parking and benches.",
        "Let me push back a little, because rent is the real problem for most of the owners I have talked to.",
        "Before we wrap up, tell our listeners where they can find your work and the map you made of the empty storefronts.",
        "Thanks again for coming on. That is it for this week, and remember to say hello to your neighbors.",
    ];
    let guest_lines = ["Mostly retired folks and parents with strollers, which is exactly who the shops should be designing for.", "Rent matters, sure, but the town owns half of those buildings, so it has more power than people think."];
    let song = media::audio_in::decode(std::path::Path::new(r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3")).unwrap();
    let music: Vec<f32> = song.0.chunks(song.2 as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect::<Vec<_>>();
    let music = stems::resample(&music[..(song.1 as usize * 12).min(music.len())], song.1, RATE);
    let mut ep: Vec<f32> = music.iter().map(|x| x * 0.8).collect();
    let gap = vec![0f32; (RATE as f32 * 0.4) as usize];
    let mut host_clean = Vec::new();
    for (i, line) in host_lines.iter().enumerate() {
        let mut v = say(line, host);
        host_clean.extend_from_slice(&v);
        // Music under the host's first line.
        if i == 0 {
            for (k, x) in v.iter_mut().enumerate() {
                *x += music.get(music.len() / 2 + k).copied().unwrap_or(0.0) * 0.25;
            }
        }
        ep.extend(&gap);
        ep.extend(v);
        if let Some(g) = guest_lines.get(i % 3).filter(|_| i % 3 < 2 && i > 0) {
            ep.extend(&gap);
            ep.extend(say(g, guest));
        }
    }
    let guest_clean: Vec<f32> = guest_lines.iter().flat_map(|g| say(g, guest)).collect();
    let path = std::env::temp_dir().join(format!("sulcusai-podcast-{}.wav", uuid::Uuid::new_v4().simple()));
    std::fs::write(&path, crate::audio::wav_encode(&ep, RATE)).unwrap();
    println!("episode: {:.0} s", ep.len() as f32 / RATE as f32);

    let (take, words, several) = own_voice::prepare_for_test(&state, &path).await.unwrap();
    let _ = std::fs::remove_file(&path);
    println!("kept {:.1} s; several: {several}; heard: {words}", take.len() as f32 / 48_000.0);
    assert!(several, "the guest was noticed");
    assert!(!words.is_empty(), "its words were heard");
    let print = |x: &[f32], rate: u32| crate::diarize::embed(&state, &crate::audio::resample(x, rate, 16_000)).unwrap();
    let (h, g) = (print(&host_clean, RATE), print(&guest_clean, RATE));
    let kept = print(&take, 48_000);
    println!("kept is like the host {:.2}, the guest {:.2}", crate::diarize::cosine(&kept, &h), crate::diarize::cosine(&kept, &g));
    assert!(crate::diarize::cosine(&kept, &h) > crate::diarize::cosine(&kept, &g) + 0.2, "the host was kept");

    // A voice from it alone, reading new words: like the host.
    let voice = own_voice::add_for_test(&state, "Host", &[("added-1", take, &words)]);
    let read = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: "The farmers market moves to the old train station next month, and the bakery will finally have a real oven.".into(), voice: Some(format!("own:{voice}")), language: Some("en".into()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    let wav = store::read(&state.paths, &state.work_cipher().unwrap(), &read.id).unwrap();
    let pcm = crate::audio::wav_decode(&wav).unwrap();
    std::fs::write(r"C:\dev\tools\voice\tests\e2e-from-podcast.wav", &wav).unwrap();
    let said = print(&pcm.samples, pcm.rate);
    let (to_host, to_guest) = (crate::diarize::cosine(&said, &h), crate::diarize::cosine(&said, &g));
    println!("narration is like the host {to_host:.2}, the guest {to_guest:.2}");
    assert!(to_host > to_guest + 0.15, "it reads like the host");
    crate::speech::stop(&state).await;
}

/// A voice from a real podcast episode alone (SULCUSAI_PODCAST, a long
/// recording of one person talking): what's kept, its words, and new words
/// read in it compared by voice print with other stretches of the episode.
/// The reading goes to C:\dev\tools\voice\tests\from-podcast.wav.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_voice_from_a_real_podcast() {
    use crate::media::{self, own_voice, store, Request};
    let path = std::path::PathBuf::from(std::env::var("SULCUSAI_PODCAST").expect("SULCUSAI_PODCAST"));
    let state = media_state();
    let never = std::sync::atomic::AtomicBool::new(false);
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &never).await.unwrap();
    let started = std::time::Instant::now();
    let (take, words, several) = own_voice::prepare_for_test(&state, &path).await.unwrap();
    println!("prepared in {:.0} s: kept {:.1} s; others talk: {several}; heard: {words}", started.elapsed().as_secs_f64(), take.len() as f32 / 48_000.0);
    assert!(!words.is_empty());
    let voice = own_voice::add_for_test(&state, "Me", &[("added-1", take, &words)]);
    let started = std::time::Instant::now();
    let read = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: "Hey everyone, welcome back. Today I want to talk about why most penny stocks fall apart, and the three things I always check before I buy one.".into(), voice: Some(format!("own:{voice}")), language: Some("en".into()), ..Default::default() })
        .await
        .unwrap()
        .remove(0);
    println!("read {:.1} s in {:.0} s", read.seconds, started.elapsed().as_secs_f64());
    let wav = store::read(&state.paths, &state.work_cipher().unwrap(), &read.id).unwrap();
    std::fs::write(r"C:\dev\tools\voice\tests\from-podcast.wav", &wav).unwrap();
    // Voice prints of three other stretches of the episode against the reading.
    let (pcm, rate, ch) = media::audio_in::decode(&path).unwrap();
    let mono: Vec<f32> = pcm.chunks(ch.max(1) as usize).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect();
    drop(pcm);
    let print = |x: &[f32], r: u32| crate::diarize::embed(&state, &crate::audio::resample(x, r, 16_000)).unwrap();
    let said = crate::audio::wav_decode(&wav).unwrap();
    let mine = print(&said.samples, said.rate);
    let len = mono.len();
    let sims: Vec<f32> = [0.2, 0.5, 0.8].iter().map(|f| {
        let at = (len as f64 * f) as usize;
        crate::diarize::cosine(&mine, &print(&mono[at..(at + rate as usize * 12).min(len)], rate))
    }).collect();
    println!("the reading against the episode's own voice: {sims:?}");
    // And a stranger's voice, for scale.
    let other = tokio::task::spawn_blocking(|| crate::tts::synthesize("Hey everyone, welcome back to the show today.", Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0)).await.unwrap().unwrap();
    let stranger = print(&other.samples.iter().map(|x| *x as f32 / 32768.0).collect::<Vec<_>>(), other.rate);
    println!("a stranger against the episode: {:.2}", crate::diarize::cosine(&stranger, &print(&mono[len / 2..len / 2 + rate as usize * 12], rate)));
    assert!(sims.iter().sum::<f32>() / 3.0 > 0.45, "the reading sounds like the episode's speaker");
    crate::speech::stop(&state).await;
}

/// Speech recognition end to end, without speakers or a microphone: a
/// Windows voice speaks a sentence and whisper.cpp writes it down.
/// SULCUSAI_SPEECH_MODEL picks the model (default: the one this PC is offered first).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_speech_round_trip() {
    use crate::speech;
    let state = speech_state();
    let voices = crate::tts::voices().unwrap();
    println!("voices: {:?}", voices.iter().map(|v| format!("{} ({})", v.name, v.language)).collect::<Vec<_>>());
    assert!(!voices.is_empty(), "no Windows voices");

    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let id = std::env::var("SULCUSAI_SPEECH_MODEL")
        .unwrap_or_else(|_| speech::recommended(&state.catalog.speech.models, &hw, &b).unwrap().id.clone());
    let spec = state.catalog.speech.models.iter().find(|m| m.id == id).unwrap().clone();
    println!("model {}: estimated {}x real time on {} threads", spec.name, speech::fit(&spec, &hw, &b).speed, speech::threads(&hw));

    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let speed = speech::install_for_test(&state, &spec, &cancel).await.unwrap();
    println!("installed and self-tested in {:.1}s; measured {speed:?}x real time", started.elapsed().as_secs_f64());
    assert!(speed.is_some_and(|s| s > 0.2));

    let ep = speech::endpoint(&state, speech::Use::Accurate).await.unwrap();
    // Without the secret prefix the server has nothing to offer.
    let port = ep.port_for_test();
    let bare = net::local_client().get(format!("http://127.0.0.1:{port}/health")).send().await.unwrap();
    assert_eq!(bare.status().as_u16(), 404, "speech server answered without its path prefix");
    let page = net::local_client().get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
    assert_eq!(page.status().as_u16(), 404, "speech server served a page without its path prefix");

    let say = "Please remind me to call the dentist on Wednesday at three thirty.";
    let s = tokio::task::spawn_blocking(move || crate::tts::synthesize(say, Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0)).await.unwrap().unwrap();
    let floats: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
    let samples = crate::audio::resample(&floats, s.rate, crate::audio::RATE);
    let started = std::time::Instant::now();
    let t = speech::transcribe(&ep, &samples, &speech::Options::default()).await.unwrap();
    println!(
        "heard {:.1}s of speech in {:.2}s ({:?}): {:?}",
        samples.len() as f64 / 16000.0,
        started.elapsed().as_secs_f64(),
        t.language,
        t.text
    );
    let lower = t.text.to_lowercase();
    assert!(lower.contains("dentist") && lower.contains("wednesday"), "{}", t.text);
    assert!(t.segments.iter().all(|s| s.end >= s.start));

    // Silence gives nothing back, not a made-up sentence.
    let quiet = vec![0.0f32; crate::audio::RATE as usize * 3];
    let t = speech::transcribe(&ep, &quiet, &speech::Options::default()).await.unwrap();
    println!("silence heard as: {:?}", t.text);
    assert!(t.text.trim().is_empty(), "{}", t.text);

    speech::stop(&state).await;
}

/// Dictation as the app runs it, with synthesized speech streamed in small
/// chunks the way a microphone delivers them (no real microphone used).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_dictation_streams_phrases() {
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    let state = speech_state();
    // Registers the quick model (downloads it the first time).
    let spec = state.catalog.speech.models.iter().find(|m| m.id == "whisper-small").unwrap().clone();
    crate::speech::install_for_test(&state, &spec, &AtomicBool::new(false)).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Live).await.unwrap();
    let speak = |text: &'static str| {
        let s = crate::tts::synthesize(text, Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0).unwrap();
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        crate::audio::resample(&f, s.rate, crate::audio::RATE)
    };
    let mut stream = vec![0.0005f32; 16_000];
    stream.extend(speak("First, buy oat milk and coffee filters."));
    stream.extend(vec![0.0005f32; 24_000]);
    stream.extend(speak("Second, book a table for four on Saturday."));
    stream.extend(vec![0.0005f32; 8_000]);

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let feeder = tokio::spawn(async move {
        for chunk in stream.chunks(320) {
            tx.send(chunk.to_vec()).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    });
    let stop = AtomicBool::new(false);
    let texts: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let emit = |kind: &str, v: serde_json::Value| {
        if kind == "text" {
            texts.lock().unwrap().push(v["text"].as_str().unwrap().to_string());
        }
    };
    let run = crate::voice::dictate(&ep, rx, &stop, &|| 0.0, &emit);
    let started = std::time::Instant::now();
    let (r, _) = tokio::join!(run, feeder);
    r.unwrap();
    println!("dictation finished {:.1}s after the audio started", started.elapsed().as_secs_f64());
    let texts = texts.into_inner().unwrap();
    println!("phrases: {texts:?}");
    assert!(texts.len() >= 2, "expected the two sentences as separate phrases");
    let all = texts.join(" ").to_lowercase();
    assert!(all.contains("oat milk") && all.contains("saturday"), "{all}");
    crate::speech::stop(&state).await;
}

/// Meeting mode end to end, without devices: two synthetic channels (the
/// call, and the user with some of the call echoing into their mic), live
/// transcription, echo removal, and notes from the real chat model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_meeting_records_transcribes_and_writes_notes() {
    use crate::meeting::{self, Input, MeetingData, Speaker};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let state = speech_state();
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    {
        let spec = state.catalog.model("qwen3-1.7b").unwrap();
        let v = spec.variant("Q4_K_M").unwrap();
        let path = state.paths.models.join(&spec.id).join(&v.file);
        let conn = state.db.lock().unwrap();
        crate::db::save_installed(&conn, &crate::db::InstalledModel {
            model_id: spec.id.clone(), quant: v.quant.clone(), path: path.display().to_string(), size: v.size, installed_at: 0, tps: None, prompt_tps: None,
        }).unwrap();
        crate::db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone())).unwrap();
    }
    let cipher = state.cipher().unwrap();
    let m = meeting::create(&state.db.lock().unwrap(), &cipher, &MeetingData { title: "Test".into(), ..Default::default() }).unwrap();

    let speak = |text: &str, voice: usize| {
        let voices = crate::tts::voices().unwrap();
        let s = crate::tts::synthesize(text, Some(&voices[voice % voices.len()].id), Some("en"), 1.0).unwrap();
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        crate::audio::resample(&f, s.rate, crate::audio::RATE)
    };
    let quiet = |secs: f32| vec![0.0003f32; (secs * 16_000.0) as usize];
    let line1 = speak("Thanks for joining everyone. We decided to move the product launch to Thursday the twelfth.", 1);
    let line2 = speak("Jordan, please send the updated budget to the whole team by Friday.", 1);
    let mine = speak("Sounds good. I will book the venue for the launch party next week.", 0);
    // The call: line 1, a pause, line 2, then silence while the user talks.
    let mut others = quiet(1.0);
    others.extend(&line1);
    others.extend(quiet(1.5));
    let echo_at = others.len();
    others.extend(&line2);
    others.extend(quiet(1.5));
    let reply_at = others.len();
    others.extend(quiet(mine.len() as f32 / 16_000.0 + 2.0));
    // The mic: quiet, then line 2 coming back from the speakers, then the user.
    let mut you = quiet(echo_at as f32 / 16_000.0);
    you.extend(line2.iter().map(|s| s * 0.35));
    you.resize(reply_at, 0.0003);
    you.extend(&mine);
    you.resize(others.len(), 0.0003);

    let (tx_o, rx_o) = tokio::sync::mpsc::unbounded_channel();
    let (tx_y, rx_y) = tokio::sync::mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = {
        let stop = stop.clone();
        tokio::spawn(async move {
            for (a, b) in others.chunks(1600).zip(you.chunks(1600)) {
                tx_o.send(a.to_vec()).unwrap();
                tx_y.send(b.to_vec()).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            stop.store(true, Ordering::SeqCst);
        })
    };
    let log: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let emit: meeting::Emit = {
        let log = log.clone();
        Arc::new(move |kind: &str, v: serde_json::Value| {
            if kind != "level" {
                log.lock().unwrap().push(format!("{kind}: {v}"));
            }
        })
    };
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    let inputs = vec![Input { speaker: Speaker::Others, rx: rx_o }, Input { speaker: Speaker::You, rx: rx_y }];
    let started = std::time::Instant::now();
    meeting::record(&state, &cipher, &m.id, ep, inputs, true, None, &stop, &|| serde_json::json!({}), emit.clone()).await.unwrap();
    feeder.await.unwrap();
    println!("recorded and transcribed in {:.1}s", started.elapsed().as_secs_f64());
    for l in log.lock().unwrap().iter() {
        println!("  {}", &l[..l.len().min(220)]);
    }
    let segs = meeting::segments(&state.db.lock().unwrap(), &cipher, &m.id);
    let text = meeting::transcript_text(&segs, &Default::default());
    println!("transcript:\n{text}");
    let yours: Vec<_> = segs.iter().filter(|s| s.speaker == Speaker::You).collect();
    assert!(yours.iter().all(|s| !s.text.to_lowercase().contains("budget")), "the echoed line should be dropped from You");
    assert!(yours.iter().any(|s| s.text.to_lowercase().contains("venue")));
    assert!(segs.iter().any(|s| s.speaker == Speaker::Others && s.text.to_lowercase().contains("budget")));

    // A clip of the stored (encrypted) audio plays back the right length.
    let wav = meeting::clip(&meeting::audio_dir(&state, &m.id), &cipher, 1.0, 4.0);
    assert_eq!(crate::audio::wav_decode(&wav).unwrap().samples.len(), 3 * 16_000);

    let started = std::time::Instant::now();
    meeting::finish(&state, &cipher, &m.id, &emit).await.unwrap();
    let detail = meeting::list(&state.db.lock().unwrap(), &cipher).into_iter().find(|x| x.id == m.id).unwrap();
    println!("notes in {:.1}s: {:#?}", started.elapsed().as_secs_f64(), detail.data);
    let notes = detail.data.notes.expect("notes");
    assert!(!notes.summary.is_empty());
    let actions = serde_json::to_string(&notes.action_items).unwrap().to_lowercase();
    assert!(actions.contains("budget"), "{actions}");
    assert_eq!(detail.status, "done");
    crate::speech::stop(&state).await;
    state.engine.lock().await.stop().await;
    let _ = std::fs::remove_dir_all(meeting::audio_dir(&state, &m.id));
}

/// Narration as the Studio's tab starts it, with no voice picked (the
/// first on the PC) and with a Windows voice: each read and kept as a
/// sound of about the right length.
///     cargo test e2e_narration_reads -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_narration_reads() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let text = "Welcome to the studio. This is a short test of reading text aloud, about ten seconds long, to check that narration works.";
    for voice in [None, Some(crate::tts::WINDOWS_DEFAULT.to_string())] {
        let t = std::time::Instant::now();
        let got = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: text.into(), voice: voice.clone(), ..Default::default() }).await;
        match &got {
            Ok(items) => println!("voice {voice:?}: {:.1} s of speech in {:.1} s", items[0].seconds, t.elapsed().as_secs_f64()),
            Err(e) => println!("voice {voice:?}: FAILED {e}"),
        }
        let items = got.unwrap();
        assert!(items[0].seconds > 4.0 && items[0].seconds < 30.0, "{}", items[0].seconds);
    }
}

/// Lists audio devices without opening any (no recording, no sound).
#[test]
#[ignore]
fn e2e_audio_devices_are_listed() {
    let inputs = crate::audio::devices(crate::audio::Flow::Input).unwrap();
    let outputs = crate::audio::devices(crate::audio::Flow::Output).unwrap();
    println!("inputs: {inputs:#?}\noutputs: {outputs:#?}");
    assert!(outputs.iter().any(|d| d.default), "a default output device");
}

/// Natural voices end to end: install (runtime + Supertonic 3), speak in two
/// languages, and check speech recognition understands what was said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_natural_voice_is_understood() {
    let state = speech_state();
    let started = std::time::Instant::now();
    crate::natural::install_for_test(&state).await.unwrap();
    println!("installed natural voices in {:.1}s", started.elapsed().as_secs_f64());
    let voices = crate::tts::voices().unwrap();
    assert!(voices.iter().any(|v| v.id == "supertonic:F1"));
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &std::sync::atomic::AtomicBool::new(false)).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    for (lang, voice, text, words) in [
        ("en", "supertonic:F2", "The quarterly report is due on Friday, so please send your numbers by Wednesday.", ["quarterly", "friday", "wednesday"]),
        ("de", "supertonic:M1", "Das Treffen beginnt morgen um neun Uhr im großen Konferenzraum.", ["treffen", "morgen", "konferenzraum"]),
    ] {
        let t0 = std::time::Instant::now();
        let s = tokio::task::spawn_blocking(move || crate::tts::synthesize(text, Some(voice), Some(lang), 1.0)).await.unwrap().unwrap();
        let secs = s.samples.len() as f64 / s.rate as f64;
        println!("{lang}: {secs:.1}s of speech made in {:.2}s ({} Hz)", t0.elapsed().as_secs_f64(), s.rate);
        assert!(secs > 2.0 && secs < 15.0);
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        let samples = crate::audio::resample(&f, s.rate, crate::audio::RATE);
        let heard = crate::speech::transcribe(&ep, &samples, &crate::speech::Options { language: Some(lang.into()), ..Default::default() }).await.unwrap();
        println!("  heard: {}", heard.text);
        let lower = heard.text.to_lowercase();
        for w in words {
            assert!(lower.contains(w), "{lang}: expected “{w}” in “{}”", heard.text);
        }
    }
    crate::speech::stop(&state).await;
}

/// Speaker labels: two different voices take turns on the call channel; the
/// transcript should label them as two consistent, different speakers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_meeting_tells_speakers_apart() {
    use crate::meeting::{self, Input, MeetingData, Speaker};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let state = speech_state();
    crate::natural::refresh(&state.paths, &state.catalog.voices);
    crate::diarize::install_for_test(&state).await.unwrap();
    assert!(crate::diarize::available(&state));
    let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
    crate::speech::install_for_test(&state, &turbo, &AtomicBool::new(false)).await.unwrap();
    let cipher = state.cipher().unwrap();
    let m = meeting::create(&state.db.lock().unwrap(), &cipher, &MeetingData { title: "Two voices".into(), ..Default::default() }).unwrap();

    let windows_male = crate::tts::voices().unwrap().into_iter().find(|v| v.engine == "system" && v.gender == "male").unwrap().id;
    let say = |text: &str, voice: &str| {
        let s = crate::tts::synthesize(text, Some(voice), Some("en"), 1.0).unwrap();
        let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
        crate::audio::resample(&f, s.rate, crate::audio::RATE)
    };
    let lines = [
        ("supertonic:F2", "Good morning, I wanted to start with the marketing budget for next quarter."),
        (windows_male.as_str(), "Sure. I think we should move some money from print ads to online campaigns."),
        ("supertonic:F2", "That makes sense, but the regional teams still rely on printed brochures."),
        (windows_male.as_str(), "Then let's keep a small print budget and review the numbers again in March."),
    ];
    let mut others = vec![0.0003f32; 16_000];
    for (voice, text) in lines {
        others.extend(say(text, voice));
        others.extend(vec![0.0003f32; 40_000]);
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = {
        let stop = stop.clone();
        tokio::spawn(async move {
            for c in others.chunks(1600) {
                tx.send(c.to_vec()).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            stop.store(true, Ordering::SeqCst);
        })
    };
    let emit: meeting::Emit = Arc::new(|kind: &str, v: serde_json::Value| {
        if kind == "relabeled" || kind == "warning" {
            println!("{kind}: {v}");
        }
    });
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Accurate).await.unwrap();
    meeting::record(&state, &cipher, &m.id, ep, vec![Input { speaker: Speaker::Others, rx }], false, None, &stop, &|| serde_json::json!({}), emit).await.unwrap();
    feeder.await.unwrap();
    let segs = meeting::segments(&state.db.lock().unwrap(), &cipher, &m.id);
    println!("{}", meeting::transcript_text(&segs, &Default::default()));
    let label_of = |needle: &str| segs.iter().find(|s| s.text.to_lowercase().contains(needle)).and_then(|s| s.voice);
    let (a1, b1, a2, b2) = (label_of("marketing"), label_of("online"), label_of("regional"), label_of("march"));
    println!("labels: {a1:?} {b1:?} {a2:?} {b2:?}");
    assert!(a1.is_some() && b1.is_some());
    assert_eq!(a1, a2, "the first voice keeps its label");
    assert_eq!(b1, b2, "the second voice keeps its label");
    assert_ne!(a1, b1, "two voices, two labels");
    assert_eq!(a1, Some(1), "numbered by who spoke first");
    crate::speech::stop(&state).await;
}

/// How much a voice's pitch moves: the spread (standard deviation, in
/// semitones) of its pitch over voiced 40 ms frames, by autocorrelation
/// between 70 and 500 Hz. Flat reading is about 1.5 or less.
#[cfg(windows)]
fn liveliness(file: &std::path::Path) -> String {
    let (samples, rate, channels) = crate::media::audio_in::decode(file).unwrap();
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    let x = crate::media::stems::resample(&mono, rate, 16_000);
    let (frame, hop, lo, hi) = (640usize, 320usize, 16_000 / 500, 16_000 / 70);
    let loudest = x.iter().map(|v| v.abs()).fold(0f32, f32::max).max(1e-6);
    let mut semis = Vec::new();
    let mut at = 0;
    while at + frame + hi < x.len() {
        let w = &x[at..at + frame + hi];
        let r0: f32 = w[..frame].iter().map(|v| v * v).sum();
        if (r0 / frame as f32).sqrt() > loudest * 0.05 {
            let (lag, r) = (lo..hi).map(|lag| (lag, (0..frame).map(|i| w[i] * w[i + lag]).sum::<f32>())).fold((0, 0f32), |b, x| if x.1 > b.1 { x } else { b });
            if lag > 0 && r > 0.5 * r0 {
                semis.push(12.0 * (16_000.0 / lag as f32 / 100.0).log2());
            }
        }
        at += hop;
    }
    if semis.len() < 10 {
        return "too little voiced sound to measure".into();
    }
    let mean = semis.iter().sum::<f32>() / semis.len() as f32;
    let sd = (semis.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / semis.len() as f32).sqrt();
    format!("pitch moves {sd:.1} semitones around {:.0} Hz", 100.0 * 2f32.powf(mean / 12.0))
}

/// The middle of the notes a pitch track holds (half are lower).
#[cfg(windows)]
fn median_note(f0: &[f32]) -> String {
    let mut v: Vec<f32> = f0.iter().copied().filter(|x| *x > 1.0).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    crate::media::svc::note_name(v.get(v.len() / 2).copied().unwrap_or(0.0))
}

/// How close a recording sounds to a person: the cosine of their CAMPPlus
/// voice prints (1 is the same voice; different people are usually under
/// 0.5). A song's voice is taken out of the band first.
#[cfg(windows)]
struct Likeness {
    conv: crate::media::svc::Converter,
    dll: std::path::PathBuf,
    separator: std::path::PathBuf,
    card: Option<crate::natural::Card>,
    person: Vec<f32>,
}

#[cfg(windows)]
impl Likeness {
    fn new(state: &crate::AppState, person_44k: &[f32]) -> Likeness {
        let real = Paths::new(app_data_dir()).unwrap();
        let rt = real.engines.join("onnxruntime-1.30.0-cuda13");
        let dll = [rt.join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
        let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
        let separator = state.paths.models.join("media").join("helpers").join("htdemucs_fp16weights.onnx");
        let conv = crate::media::svc::Converter::new(&dll, &state.paths.models.join("media").join("own-voice-sing"), 8, card).unwrap();
        let mut l = Likeness { conv, dll, separator, card, person: Vec::new() };
        l.person = l.print(person_44k);
        l
    }

    fn print(&mut self, x44: &[f32]) -> Vec<f32> {
        let w16 = crate::media::stems::resample(x44, crate::media::svc::RATE, 16_000);
        let (n, fb) = crate::diarize::fbank_with(&w16, true);
        let e = self.conv.run("campplus.onnx", vec![("fbank", vec![1, n as i64, 80], fb)]).unwrap();
        let norm = e.iter().map(|v| v * v).sum::<f32>().sqrt();
        e.into_iter().map(|v| v / norm).collect()
    }

    fn of(&mut self, file: &std::path::Path, song: bool) -> f32 {
        use crate::media::stems;
        let (samples, rate, channels) = crate::media::audio_in::decode(file).unwrap();
        let ch = channels.max(1) as usize;
        let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
        let voice = if song {
            let mix = stems::Stereo { rate, ch: [mono.clone(), mono] }.resampled(stems::RATE);
            let (parts, _) = stems::separate(&self.dll, &self.separator, &mix, 8, &[3], self.card, &std::sync::atomic::AtomicBool::new(false)).unwrap();
            stems::unload();
            stems::resample(&parts[0].mono(), stems::RATE, crate::media::svc::RATE)
        } else {
            stems::resample(&mono, rate, crate::media::svc::RATE)
        };
        let p = self.print(&voice);
        p.iter().zip(&self.person).map(|(a, b)| a * b).sum()
    }
}

/// The installed app's recorded voice (read only, copied into a test
/// state) through the app's own jobs: narration of lines said different
/// ways, a song sung again in the voice, and a custom model trained on the
/// voice's singing with a song made on it. Beside each, how much its pitch
/// moves, next to the person's own recordings. The files go to
/// C:\dev\tools\voice\for-you.
///     cargo test e2e_your_voice_end_to_end -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_your_voice_end_to_end() {
    use crate::media::{self, own_voice, store, train, MusicSpec, Request};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let real = Paths::new(app_data_dir()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&real.data.join("keys.json"), crate::crypto::platform_protector(&real.data)).unwrap();
    let rc = vault.work_cipher().expect("the app's data key is locked with a password");
    let voice = own_voice::list(&conn, &rc).into_iter().next().expect("a recorded voice");
    drop(conn);
    let vid = own_voice::copy_for_test(&real, &rc, &voice, &state);
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\for-you");
    std::fs::create_dir_all(&out).unwrap();
    let save = |name: &str, id: &str| {
        let file = out.join(name);
        std::fs::write(&file, store::read(&state.paths, &c, id).unwrap()).unwrap();
        file
    };
    let only = std::env::var("SULCUSAI_ONLY").unwrap_or_default();
    let wanted = |part: &str| only.is_empty() || only.split(',').any(|p| p == part);
    let mut rows = vec![format!("{}: {} takes, {:.0} s sung", voice.name, voice.takes.len(), own_voice::sung_takes(&state.paths, &c, &own_voice::get(&state.db.lock().unwrap(), &c, &vid).unwrap()).unwrap().iter().map(|t| t.len()).sum::<usize>() as f64 / 48_000.0)];
    let (reading, _) = own_voice::speech_reference(&state.paths, &c, &own_voice::get(&state.db.lock().unwrap(), &c, &vid).unwrap()).unwrap();
    let mine = out.join("your-reading.wav");
    std::fs::write(&mine, crate::audio::wav_encode(&reading, 24_000)).unwrap();
    rows.push(format!("your own reading: {}", liveliness(&mine)));

    let (singing, _) = own_voice::singing_reference(&state.paths, &c, &own_voice::get(&state.db.lock().unwrap(), &c, &vid).unwrap()).unwrap();
    let mut like = Likeness::new(&state, &singing);
    if let Some(s) = own_voice::all_singing(&state.paths, &c, &own_voice::get(&state.db.lock().unwrap(), &c, &vid).unwrap()) {
        if let Some((lo, hi)) = crate::media::svc::range_of(&like.conv.pitch_track(&s).unwrap()) {
            rows.push(format!("your singing range as the app reads it: {} to {}", crate::media::svc::note_name(lo), crate::media::svc::note_name(hi)));
        }
    }
    rows.push(format!("your reading against your singing: likeness {:.2}", like.of(&mine, false)));
    if wanted("narrate") {
        let script = "I can't believe it actually worked! This is the best news I've had all week.\nHmm. I'm not so sure about that. Let me think it over before we decide.\nOkay, listen closely: the meeting starts at nine, not ten, so please don't be late.\nWait, are you really coming with us tonight? That's amazing!";
        let t = std::time::Instant::now();
        let read = media::run(&state, store::new_id(), Request { op: "narrate".into(), prompt: script.into(), voice: Some(format!("own:{vid}")), language: Some("en".into()), ..Default::default() }).await.unwrap().remove(0);
        let file = save("narration.wav", &read.id);
        rows.push(format!("narration: {:.0} s; {}; likeness {:.2}", t.elapsed().as_secs_f64(), liveliness(&file), like.of(&file, false)));
    }
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let music = std::env::var("SULCUSAI_MUSIC_MODEL").unwrap_or_else(|_| "ace-step-1.5-xl".into());
    if wanted("song") {
        media::install_for_test(&state, &music).await.unwrap();
        let t = std::time::Instant::now();
        let song = media::run(&state, store::new_id(), Request { op: "music".into(), model_id: Some(music.clone()), prompt: "A warm pop song about a fresh start, acoustic guitar, light drums, catchy chorus".into(), lyrics: Some(lyrics.into()), seconds: Some(60.0), seed: Some(5), language: Some("en".into()), music: Some(MusicSpec::default()), ..Default::default() }).await.unwrap().remove(0);
        let made = save("song-as-made.mp3", &song.id);
        let sung = media::run(&state, store::new_id(), Request { op: "revoice".into(), source: Some(song.id.clone()), voice: Some(vid.clone()), ..Default::default() }).await.unwrap().remove(0);
        let file = save("song-in-your-voice.wav", &sung.id);
        rows.push(format!("song: {:.0} s for both; likeness as made {:.2}, sung again in your voice {:.2}", t.elapsed().as_secs_f64(), like.of(&made, true), like.of(&file, true)));
    }
    if wanted("own") {
        // A new song in your voice the app's way: a singer of your sex, then
        // sung again in your voice and range.
        media::install_for_test(&state, &music).await.unwrap();
        let t = std::time::Instant::now();
        let spec = MusicSpec { own_voice: Some(vid.clone()), ..Default::default() };
        let song = media::run(&state, store::new_id(), Request { op: "music".into(), model_id: Some(music.clone()), prompt: "A warm pop song about a fresh start, acoustic guitar, light drums, catchy chorus".into(), lyrics: Some(lyrics.into()), seconds: Some(60.0), seed: Some(5), language: Some("en".into()), music: Some(spec), ..Default::default() }).await.unwrap().remove(0);
        let file = save("song-in-your-voice-from-the-start.wav", &song.id);
        rows.push(format!("new song in your voice: {:.0} s; likeness {:.2}", t.elapsed().as_secs_f64(), like.of(&file, true)));
    }
    if wanted("train") {
        let tc = state.catalog.media.trainer.clone().unwrap();
        let source = train::source_dir(&state.paths, &tc);
        if !source.join("train.py").is_file() {
            let zip = std::path::PathBuf::from(r"C:\dev\sulcusai-dist\music").join(format!("ace-step-1.5-trainer-{}.zip", tc.build));
            zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap().extract(&source).unwrap();
        }
        let turbo = state.catalog.speech.models.iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap().clone();
        crate::speech::install_for_test(&state, &turbo, &std::sync::atomic::AtomicBool::new(false)).await.unwrap();
        media::install_for_test(&state, train::BASE_MODEL).await.unwrap();
        let t = std::time::Instant::now();
        let spec = train::TrainSpec { name: "Your voice".into(), description: "solo singer, warm clear lead vocal".into(), files: Vec::new(), songs: Vec::new(), voice: Some(vid.clone()) };
        media::run(&state, store::new_id(), Request { op: "train".into(), prompt: "Your voice".into(), train: Some(spec), ..Default::default() }).await.unwrap();
        let cm = train::list(&state.db.lock().unwrap(), &c).into_iter().next().expect("a custom model");
        let song = media::run(&state, store::new_id(), Request { op: "music".into(), prompt: "A warm pop song about a fresh start".into(), lyrics: Some(lyrics.into()), seconds: Some(60.0), seed: Some(5), language: Some("en".into()), music: Some(MusicSpec { custom: Some(cm.id.clone()), ..Default::default() }), ..Default::default() }).await.unwrap().remove(0);
        let file = save("song-custom-model.mp3", &song.id);
        rows.push(format!("custom model of your singing: {:.0} s to train and sing ({} songs, {} passes); likeness {:.2}", t.elapsed().as_secs_f64(), cm.songs, cm.epochs, like.of(&file, true)));
        train::remove(&state.db.lock().unwrap(), &c, &state.paths, &cm.id).unwrap();
    }
    println!("\n{}", rows.join("\n"));
}

/// What can be told of the voice files sent before, and of the installed
/// app's recorded voice, by measuring rather than listening: each take's
/// sung range (RMVPE, the lowest and highest 5% left out), and for each song
/// the range and steadiness of the voice taken out of the band, its
/// loudness against the band, and the words Whisper hears.
///     cargo test e2e_your_voice_listen -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_your_voice_listen() {
    use crate::media::{own_voice, stems, svc};
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&real.data.join("keys.json"), crate::crypto::platform_protector(&real.data)).unwrap();
    let rc = vault.work_cipher().unwrap();
    let voice = own_voice::list(&conn, &rc).into_iter().next().expect("a recorded voice");
    let rt = real.engines.join("onnxruntime-1.30.0-cuda13");
    let dll = [rt.join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
    let mut conv = svc::Converter::new(&dll, &state.paths.models.join("media").join("own-voice-sing"), 8, card).unwrap();
    let span = |f0: &[f32]| match svc::range_of(f0) {
        Some((lo, hi)) => format!("{} to {} ({:.0} semitones)", svc::note_name(lo), svc::note_name(hi), 12.0 * (hi / lo).log2()),
        None => "no singing heard".into(),
    };
    // How much the pitch wobbles from one 10 ms to the next within a held
    // note (cents): a converted voice that shakes or warbles shows here.
    let wobble = |f0: &[f32]| {
        let d: Vec<f32> = f0.windows(2).filter(|w| w[0] > 1.0 && w[1] > 1.0).map(|w| 1200.0 * (w[1] / w[0]).log2().abs()).filter(|c| *c < 100.0).collect();
        if d.is_empty() { 0.0 } else { d.iter().sum::<f32>() / d.len() as f32 }
    };
    let mut all_sung = Vec::new();
    println!("\n{}'s takes:", voice.name);
    for (line, kind, samples) in own_voice::takes_for_test(&real, &rc, &voice) {
        let x = stems::resample(&samples, 48_000, svc::RATE);
        let f0 = conv.pitch_track(&x).unwrap();
        let voiced = f0.iter().filter(|v| **v > 1.0).count() as f32 / f0.len().max(1) as f32;
        println!("  {line} ({kind:?}, {:.1} s): {}, voiced {:.0}%, wobble {:.0} cents", samples.len() as f32 / 48_000.0, span(&f0), voiced * 100.0, wobble(&f0));
        if kind == Some(own_voice::LineKind::Sing) {
            all_sung.extend(f0);
        }
    }
    println!("  all singing: {}", span(&all_sung));
    let lyrics = "Morning light on the window Coffee warm in my hands Every road is a new start I can feel it again Sing it loud, let it carry Over rooftops and trees Sing it loud, let it carry All the way back to me";
    let wanted = sung_words(lyrics);
    let dir = std::path::PathBuf::from(r"C:\dev\tools\voice\for-you");
    let sep = state.paths.models.join("media").join("helpers").join("htdemucs_fp16weights.onnx");
    for name in ["song-as-made.mp3", "song-in-your-voice.wav", "song-in-your-voice-from-the-start.wav", "song-custom-model.mp3"] {
        let file = dir.join(name);
        let (samples, rate, channels) = crate::media::audio_in::decode(&file).unwrap();
        let ch = channels.max(1) as usize;
        let left: Vec<f32> = samples.chunks(ch).map(|f| f[0]).collect();
        let right: Vec<f32> = samples.chunks(ch).map(|f| f[ch - 1]).collect();
        let mix = stems::Stereo { rate, ch: [left, right] }.resampled(stems::RATE);
        let (parts, _) = stems::separate(&dll, &sep, &mix, 8, &[3], card, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        stems::unload();
        let vocals = stems::resample(&parts[0].mono(), stems::RATE, svc::RATE);
        let f0 = conv.pitch_track(&vocals).unwrap();
        let whole = mix.mono();
        let v44 = parts[0].mono();
        let band: Vec<f32> = whole.iter().zip(&v44).map(|(m, v)| m - v).collect();
        let band_db = 20.0 * (crate::audio::rms(&band) / crate::audio::rms(&v44).max(1e-6)).log10();
        let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
        let got = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
        println!("{name}: voice {}, middle {}, wobble {:.0} cents, band {band_db:+.1} dB against the voice, {got} of {} words heard", span(&f0), median_note(&f0), wobble(&f0), wanted.len());
        println!("  heard: {}", heard.chars().take(200).collect::<String>());
    }
}

/// The other way of choosing what a song's voice is copied from that was
/// tried: the lines that reach the most notes (soft to loud, the scale)
/// first, as much as the converter takes.
#[cfg(windows)]
fn widest_first(paths: &Paths, c: &crate::crypto::Cipher, voice: &crate::media::own_voice::OwnVoice) -> Vec<f32> {
    use crate::media::{own_voice, stems, svc};
    let takes = own_voice::takes_for_test(paths, c, voice);
    ["soft_loud", "scale"]
        .iter()
        .filter_map(|l| takes.iter().find(|t| t.0 == *l))
        .flat_map(|t| stems::resample(&t.2, 48_000, svc::RATE))
        .take((svc::MAX_PROMPT_S * svc::RATE as f32) as usize)
        .collect()
}

/// One song sung in the recorded voice in several keys, to choose how a
/// song is fitted to a singer: the voice copied from the song take first or
/// from the widest-ranging takes first, at the song's own pitch, an octave
/// down, and in keys between with the band (all but the drums) moved too;
/// then the converter's steps, guidance and noise seed in one of those keys
/// (another seed shows how much the measures move by chance alone).
/// For each: likeness to the person, how closely the new voice keeps the
/// tune, its range, and the words Whisper hears. Sings
/// C:\dev\tools\voice\for-you\song-as-made.mp3; the songs go to
/// C:\dev\tools\voice\keys. SULCUSAI_ONLY picks cases by name.
///     cargo test e2e_your_voice_keys -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_your_voice_keys() {
    use crate::media::{own_voice, stems, svc, transpose};
    use std::sync::atomic::AtomicBool;
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&real.data.join("keys.json"), crate::crypto::platform_protector(&real.data)).unwrap();
    let rc = vault.work_cipher().unwrap();
    let voice = own_voice::list(&conn, &rc).into_iter().next().expect("a recorded voice");
    drop(conn);
    let (song_first, _) = own_voice::singing_reference(&real, &rc, &voice).unwrap();
    let widest_first = widest_first(&real, &rc, &voice);
    // Likeness against all of their singing (`e2e_your_voice_judge` measures
    // against their reading, which neither way copies).
    let all = own_voice::all_singing(&real, &rc, &voice).unwrap();
    let mut like = Likeness::new(&state, &all);
    let span = |f0: &[f32]| svc::range_of(f0).map_or("no singing heard".into(), |(lo, hi)| format!("{} to {}", svc::note_name(lo), svc::note_name(hi)));
    println!("\ntheir singing: {}", span(&like.conv.pitch_track(&all).unwrap()));
    for (name, r) in [("the song take first", &song_first), ("the widest first", &widest_first)] {
        let print = like.print(r);
        println!("what is copied, {name}: {}, likeness {:.2}", span(&like.conv.pitch_track(r).unwrap()), print.iter().zip(&like.person).map(|(a, b)| a * b).sum::<f32>());
    }
    let (samples, rate, channels) = crate::media::audio_in::decode(std::path::Path::new(r"C:\dev\tools\voice\for-you\song-as-made.mp3")).unwrap();
    let ch = channels.max(1) as usize;
    let mix = stems::Stereo { rate, ch: [samples.chunks(ch).map(|f| f[0]).collect(), samples.chunks(ch).map(|f| f[ch - 1]).collect()] }.resampled(stems::RATE);
    let (parts, _) = stems::separate(&like.dll, &like.separator, &mix, 8, &[0, 1, 2, 3], like.card, &AtomicBool::new(false)).unwrap();
    stems::unload();
    let add = |a: &stems::Stereo, b: &stems::Stereo| stems::Stereo { rate: a.rate, ch: [0, 1].map(|c| a.ch[c].iter().zip(&b.ch[c]).map(|(x, y)| x + y).collect()) };
    let drums = parts[0].clone();
    let tonal = add(&parts[1], &parts[2]);
    let sung = parts[3].mono();
    let tune = like.conv.pitch_track(&sung).unwrap();
    println!("the song's voice: {}", span(&tune));
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\keys");
    std::fs::create_dir_all(&out).unwrap();
    let lyrics = "Morning light on the window Coffee warm in my hands Every road is a new start I can feel it again Sing it loud, let it carry Over rooftops and trees Sing it loud, let it carry All the way back to me";
    let wanted = sung_words(lyrics);
    let only = std::env::var("SULCUSAI_ONLY").unwrap_or_default();
    let plain = svc::Settings::default();
    let cases: [(&str, &[f32], i32, svc::Settings); 13] = [
        ("song-take-first", &song_first, 0, plain),
        ("widest-first", &widest_first, 0, plain),
        ("octave-down", &widest_first, -12, plain),
        ("down-9", &widest_first, -9, plain),
        ("down-7", &widest_first, -7, plain),
        ("down-5", &widest_first, -5, plain),
        ("down-4", &widest_first, -4, plain),
        ("down-3", &widest_first, -3, plain),
        ("song-take-first-down-5", &song_first, -5, plain),
        ("down-4-steps-50", &widest_first, -4, svc::Settings { steps: 50, ..plain }),
        ("down-4-cfg-1", &widest_first, -4, svc::Settings { cfg: 1.0, ..plain }),
        ("down-4-cfg-0.4", &widest_first, -4, svc::Settings { cfg: 0.4, ..plain }),
        ("down-4-seed-2", &widest_first, -4, svc::Settings { seed: 2, ..plain }),
    ];
    let mut rows = Vec::new();
    for (name, reference, semis, settings) in cases.into_iter().filter(|c| only.is_empty() || only.split(',').any(|o| o == c.0)) {
        let t = std::time::Instant::now();
        let band_move = semis - 12 * (semis as f32 / 12.0).round() as i32;
        // A converter of its own each time, as a job has: one kept across
        // songs fills the card.
        let mut conv = svc::Converter::new(&like.dll, &state.paths.models.join("media").join("own-voice-sing"), 8, like.card).unwrap();
        let v = conv.voice(reference).unwrap();
        let new_voice = conv.convert(&v, &sung, svc::Settings { semitones: semis, ..settings }, &AtomicBool::new(false), &|_| {}).unwrap();
        drop(conv);
        let band = add(&drums, &transpose::shift(&tonal, band_move));
        let mixed = own_voice::mix(&band, &sung, &new_voice);
        let file = out.join(format!("{name}.wav"));
        std::fs::write(&file, own_voice::wav_stereo(&mixed.ch[0], &mixed.ch[1], mixed.rate)).unwrap();
        let took = t.elapsed().as_secs_f64();
        let print = like.print(&new_voice);
        let likeness: f32 = print.iter().zip(&like.person).map(|(a, b)| a * b).sum();
        let got = like.conv.pitch_track(&new_voice).unwrap();
        let k = 2f32.powf(semis as f32 / 12.0);
        let mut cents: Vec<f32> = tune.iter().zip(&got).filter(|(a, b)| **a > 1.0 && **b > 1.0).map(|(a, b)| (1200.0 * (b / (a * k)).log2()).abs()).collect();
        cents.sort_by(|a, b| a.total_cmp(b));
        let close = cents.iter().filter(|c| **c < 50.0).count() as f32 / cents.len().max(1) as f32;
        let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
        let words = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
        rows.push(format!(
            "{name} (voice {semis:+}, band {band_move:+}): {took:.0} s; voice {}; likeness {likeness:.2}; tune kept {:.0}% within 50 cents (median off {:.0}); {words} of {} words",
            span(&got),
            close * 100.0,
            cents.get(cents.len() / 2).copied().unwrap_or(0.0),
            wanted.len()
        ));
        println!("{}", rows.last().unwrap());
    }
    println!("\n{}", rows.join("\n"));
}

/// The songs in C:\dev\tools\voice\keys judged against recordings neither
/// way of copying the voice uses (the person's reading, and their held
/// note), so no version is measured against its own prompt. Also prints the
/// key `svc::fit_key` picks for the song.
///     cargo test e2e_your_voice_judge -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_your_voice_judge() {
    use crate::media::{own_voice, stems, svc};
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&real.data.join("keys.json"), crate::crypto::platform_protector(&real.data)).unwrap();
    let rc = vault.work_cipher().unwrap();
    let voice = own_voice::list(&conn, &rc).into_iter().next().expect("a recorded voice");
    drop(conn);
    let (reading, _) = own_voice::speech_reference(&real, &rc, &voice).unwrap();
    let reading = stems::resample(&reading, own_voice::TTS_RATE, svc::RATE);
    let hold = own_voice::takes_for_test(&real, &rc, &voice).into_iter().find(|t| t.0 == "hold").map(|t| stems::resample(&t.2, 48_000, svc::RATE)).expect("a held note");
    let all = own_voice::all_singing(&real, &rc, &voice).unwrap();
    let mut like = Likeness::new(&state, &reading);
    let by_reading = like.person.clone();
    let by_hold = like.print(&hold);
    let person = svc::range_of(&like.conv.pitch_track(&all).unwrap()).unwrap();
    for (name, r) in [("the song take first", own_voice::singing_reference(&real, &rc, &voice).unwrap().0), ("the widest first", widest_first(&real, &rc, &voice))] {
        let p = like.print(&r);
        let cos = |q: &[f32]| p.iter().zip(q).map(|(a, b)| a * b).sum::<f32>();
        println!("what is copied, {name}: against their reading {:.2}, against their held note {:.2}", cos(&by_reading), cos(&by_hold));
    }
    let (samples, rate, channels) = crate::media::audio_in::decode(std::path::Path::new(r"C:\dev\tools\voice\for-you\song-as-made.mp3")).unwrap();
    let ch = channels.max(1) as usize;
    let mix = stems::Stereo { rate, ch: [samples.chunks(ch).map(|f| f[0]).collect(), samples.chunks(ch).map(|f| f[ch - 1]).collect()] }.resampled(stems::RATE);
    let (parts, _) = stems::separate(&like.dll, &like.separator, &mix, 8, &[3], like.card, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    stems::unload();
    let tune = like.conv.pitch_track(&parts[0].mono()).unwrap();
    println!("\nfit_key for this song and singer: {:?}", svc::fit_key(&tune, person));
    let mut costs = svc::key_costs(&tune, person);
    costs.sort_by(|a, b| a.1.iter().sum::<f32>().total_cmp(&b.1.iter().sum()));
    for (f, [outside, moved, off]) in costs.iter().take(8) {
        println!("  voice {:+}, band {:+}: outside {outside:.2}, moved {moved:.2}, off the middle {off:.2}", f.voice, f.band);
    }
    let mut notes: Vec<f32> = tune.iter().copied().filter(|v| *v > 1.0).collect();
    notes.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f32| svc::note_name(notes[((notes.len() - 1) as f32 * q) as usize]);
    println!("  the tune at 1, 5, 50, 95, 99%: {} {} {} {} {}", at(0.01), at(0.05), at(0.5), at(0.95), at(0.99));
    let mut sung: Vec<f32> = like.conv.pitch_track(&all).unwrap().into_iter().filter(|v| *v > 1.0).collect();
    sung.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f32| svc::note_name(sung[((sung.len() - 1) as f32 * q) as usize]);
    println!("  their singing at 1, 5, 50, 95, 99%: {} {} {} {} {}", at(0.01), at(0.05), at(0.5), at(0.95), at(0.99));
    if std::env::var("SULCUSAI_ONLY").is_ok_and(|o| o == "fit") {
        return;
    }
    let mut files: Vec<_> = std::fs::read_dir(r"C:\dev\tools\voice\keys").unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wav")).collect();
    files.sort();
    for file in files {
        let (samples, rate, channels) = crate::media::audio_in::decode(&file).unwrap();
        let ch = channels.max(1) as usize;
        let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
        let mix = stems::Stereo { rate, ch: [mono.clone(), mono] }.resampled(stems::RATE);
        let (parts, _) = stems::separate(&like.dll, &like.separator, &mix, 8, &[3], like.card, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        stems::unload();
        let p = like.print(&stems::resample(&parts[0].mono(), stems::RATE, svc::RATE));
        let cos = |q: &[f32]| p.iter().zip(q).map(|(a, b)| a * b).sum::<f32>();
        println!("{}: against their reading {:.2}, against their held note {:.2}", file.file_stem().unwrap().to_string_lossy(), cos(&by_reading), cos(&by_hold));
    }
}

/// Where the music model puts a man's voice, and whether it can be moved:
/// the same words and seed sung as "male", as a baritone, as a deep
/// baritone, and as "male" in keys 3 and 5 semitones lower. Prints each
/// voice's range (taken out of the band) and the words Whisper hears; the
/// songs go to C:\dev\tools\voice\register.
///     cargo test e2e_singer_register -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_singer_register() {
    use crate::media::{self, music, stems, store, svc, MusicSpec, Request};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let model = std::env::var("SULCUSAI_MUSIC_MODEL").unwrap_or_else(|_| "ace-step-1.5-xl".into());
    media::install_for_test(&state, &model).await.unwrap();
    let real = Paths::new(app_data_dir()).unwrap();
    let rt = real.engines.join("onnxruntime-1.30.0-cuda13");
    let dll = [rt.join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
    let mut conv = svc::Converter::new(&dll, &state.paths.models.join("media").join("own-voice-sing"), 8, card).unwrap();
    let sep = state.paths.models.join("media").join("helpers").join("htdemucs_fp16weights.onnx");
    let out = std::path::PathBuf::from(r"C:\dev\tools\voice\register");
    std::fs::create_dir_all(&out).unwrap();
    let lyrics = "[Verse 1]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees\nSing it loud, let it carry\nAll the way back to me";
    let wanted = sung_words(lyrics);
    let prompt = "A warm pop song about a fresh start, acoustic guitar, light drums, catchy chorus";
    let seed = 5i64;
    let (_, base_key, _) = music::settle(prompt, None, None, None, seed);
    let down = |semis: i32| {
        const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        let (root, mode) = base_key.split_once(' ').unwrap();
        let at = NOTES.iter().position(|n| *n == root).unwrap() as i32;
        format!("{} {mode}", NOTES[(at - semis).rem_euclid(12) as usize])
    };
    let only = std::env::var("SULCUSAI_ONLY").unwrap_or_default();
    let cases: Vec<(&str, &str, String)> = vec![
        ("male", "male", base_key.clone()),
        ("baritone", "baritone male", base_key.clone()),
        ("deep-baritone", "deep low baritone male", base_key.clone()),
        ("male-key-3", "male", down(3)),
        ("male-key-5", "male", down(5)),
        ("baritone-key-5", "baritone male", down(5)),
    ];
    let mut rows = Vec::new();
    for (name, voice, key) in cases.into_iter().filter(|c| only.is_empty() || only.split(',').any(|o| o == c.0)) {
        let spec = MusicSpec { voice: Some(voice.into()), key: Some(key.clone()), ..Default::default() };
        let t = std::time::Instant::now();
        let song = media::run(&state, store::new_id(), Request { op: "music".into(), model_id: Some(model.clone()), prompt: prompt.into(), lyrics: Some(lyrics.into()), seconds: Some(60.0), seed: Some(seed), language: Some("en".into()), music: Some(spec), ..Default::default() }).await.unwrap().remove(0);
        let file = out.join(format!("{name}.mp3"));
        std::fs::write(&file, store::read(&state.paths, &c, &song.id).unwrap()).unwrap();
        let took = t.elapsed().as_secs_f64();
        let (samples, rate, channels) = crate::media::audio_in::decode(&file).unwrap();
        let ch = channels.max(1) as usize;
        let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
        let mix = stems::Stereo { rate, ch: [mono.clone(), mono] }.resampled(stems::RATE);
        let (parts, _) = stems::separate(&dll, &sep, &mix, 8, &[3], card, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        stems::unload();
        let f0 = conv.pitch_track(&stems::resample(&parts[0].mono(), stems::RATE, svc::RATE)).unwrap();
        let (heard, _) = heard_words(&whisper_hears(&state, &file, Some("en")));
        let got = wanted.iter().filter(|w| sung_words(&heard).contains(*w)).count();
        let range = svc::range_of(&f0).map(|(lo, hi)| format!("{} to {}", svc::note_name(lo), svc::note_name(hi))).unwrap_or_default();
        let middle = median_note(&f0);
        rows.push(format!("{name} ({voice}, {key}): {took:.0} s; voice {range}, middle {middle}; {got} of {} words", wanted.len()));
        println!("{}", rows.last().unwrap());
    }
    println!("\n{}", rows.join("\n"));
}

/// The live voice changer's work, half a second at a time, on the card: a
/// natural voice reading a sentence is turned into the installed app's
/// recorded voice (read only). Prints how long each half second took (it
/// has to be under half a second to keep up) and writes what came out to
/// C:\dev\tools\voice\live.
///     cargo test e2e_live_voice_keeps_up -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_live_voice_keeps_up() {
    use crate::media::{own_voice, stems, svc};
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let conn = rusqlite::Connection::open_with_flags(&real.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&real.data.join("keys.json"), crate::crypto::platform_protector(&real.data)).unwrap();
    let rc = vault.work_cipher().expect("the app's data key is locked with a password");
    let voice = own_voice::list(&conn, &rc).into_iter().next().expect("a recorded voice");
    let (reading, _) = own_voice::speech_reference(&real, &rc, &voice).unwrap();
    let reference: Vec<f32> = stems::resample(&reading, own_voice::TTS_RATE, svc::RATE).into_iter().take(svc::RATE as usize * 4).collect();
    crate::natural::install_for_test(&state).await.unwrap();
    let said = crate::natural::synthesize("Good morning! I'd like a large coffee and two blueberry muffins, please. Thanks so much.", "M1", Some("en"), 1.0).unwrap();
    let input = stems::resample(&said.samples.iter().map(|s| *s as f32 / 32768.0).collect::<Vec<_>>(), said.rate, svc::RATE);
    let rt = real.engines.join("onnxruntime-1.30.0-cuda13");
    let dll = [rt.join("onnxruntime.dll"), real.engines.join("onnxruntime-1.30.0-cpu-x64").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = dll.to_string_lossy().contains("cuda13").then_some(crate::natural::Card { device: 0, dml: false });
    let mut conv = svc::Converter::new(&dll, &state.paths.models.join("media").join("own-voice-sing"), 8, card).unwrap();
    let v = conv.voice(&reference).unwrap();
    let settings = svc::Settings { steps: 6, cfg: 0.7, semitones: 0, seed: 7 };
    let mut noise = svc::Noise::new(7);
    use crate::media::live_voice::{BLOCK as block, CONTEXT as context, FADE as fade};
    conv.convert_live(&v, &vec![0.0; context + block + fade], block + fade, settings, &mut noise).unwrap();
    let mut heard: Vec<f32> = Vec::new();
    let (mut tail, mut out, mut took) = (Vec::new(), Vec::new(), Vec::new());
    for chunk in input.chunks(block) {
        heard.extend_from_slice(chunk);
        let keep = context + block + fade;
        let mut window = vec![0.0; keep.saturating_sub(heard.len())];
        window.extend_from_slice(&heard[heard.len().saturating_sub(keep)..]);
        let t = std::time::Instant::now();
        let piece = conv.convert_live(&v, &window, block + fade, settings, &mut noise).unwrap();
        took.push(t.elapsed().as_millis() as u64);
        out.extend(crate::media::live_voice::join(&mut tail, &piece));
    }
    let dir = std::path::PathBuf::from(r"C:\dev\tools\voice\live");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("said.wav"), crate::audio::wav_encode(&input, svc::RATE)).unwrap();
    std::fs::write(dir.join("in-your-voice.wav"), crate::audio::wav_encode(&out, svc::RATE)).unwrap();
    let mut sorted = took.clone();
    sorted.sort();
    let ms = (block * 1000 / svc::RATE as usize) as u64;
    let (words, _) = heard_words(&whisper_hears(&state, &dir.join("in-your-voice.wav"), Some("en")));
    println!("each {ms} ms piece took {took:?} ms (middle {} ms, on the card: {}); heard: {words}", sorted[sorted.len() / 2], conv.on_card);
    assert!(sorted[sorted.len() / 2] < ms * 3 / 4, "it can't keep up with room to spare");
}

/// The wake word heard the way the app hears it: a natural voice says a few
/// things, the live speech model writes them down with the phrase as its
/// hint, and only the ones with the phrase wake it.
///     cargo test e2e_the_wake_word_is_heard -- --ignored --nocapture
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_the_wake_word_is_heard() {
    let state = media_state();
    crate::natural::install_for_test(&state).await.unwrap();
    let small = state.catalog.speech.models.iter().find(|m| m.id == "whisper-small").unwrap().clone();
    crate::speech::install_for_test(&state, &small, &std::sync::atomic::AtomicBool::new(false)).await.unwrap();
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Live).await.expect("a speech model is installed");
    let opts = crate::speech::Options { prompt: Some("Hey Sulcus".into()), ..Default::default() };
    for (words, wakes) in [("Hey Sulcus.", true), ("Hey Sulcus, what's on my calendar today?", true), ("Hey Siri.", false), ("Thanks, that's all for now.", false), ("Hey, so, listen.", false)] {
        let said = crate::natural::synthesize(words, "M1", Some("en"), 1.0).unwrap();
        let samples = crate::audio::resample(&said.samples.iter().map(|s| *s as f32 / 32768.0).collect::<Vec<_>>(), said.rate, crate::audio::RATE);
        let heard = crate::speech::transcribe(&ep, &samples, &opts).await.unwrap().text;
        let woke = crate::wake::said(&heard, "Hey Sulcus");
        println!("{words:?} heard as {heard:?}: {}", if woke { "wakes" } else { "doesn't wake" });
        assert_eq!(woke, wakes, "{words}");
    }
}

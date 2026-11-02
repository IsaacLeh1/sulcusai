// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn a_voice_is_a_mans_or_a_womans_by_its_pitch() {
    // A voice with overtones, gliding a little, between pauses.
    let voice = |hz: f32| -> Vec<f32> {
        (0..16_000 * 4)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                let f = hz * (1.0 + 0.03 * (t * 2.0).sin());
                let on = if (t * 2.0).fract() < 0.8 { 1.0 } else { 0.0 };
                on * (1..5).map(|k| (std::f32::consts::TAU * f * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.2
            })
            .collect()
    };
    let speaking = |hz: f32| sex_by_pitch(middle_pitch(&voice(hz), 16_000).unwrap(), 160.0, 175.0);
    assert_eq!(speaking(120.0), Some("male"));
    assert_eq!(speaking(139.0), Some("male"));
    assert_eq!(speaking(215.0), Some("female"));
    assert_eq!(speaking(168.0), None, "in between: left to the description");
    assert_eq!(middle_pitch(&vec![0.0; 16_000], 16_000), None);
}

#[test]
fn text_goes_in_sentence_lines() {
    let l = lines_of("Hello there. How are you?\n\nFine!");
    assert_eq!(l, vec!["Hello there. How are you?", "Fine!"]);
    let long = "This sentence is about forty characters. ".repeat(12);
    let l = lines_of(&long);
    assert!(l.len() >= 2 && l.iter().all(|x| x.len() <= 330), "{l:?}");
}

#[test]
fn streamed_lines_are_split_at_their_headers() {
    let head = |n: usize| {
        let mut h = b"RIFF\xff\xff\xff\x7fWAVEfmt ".to_vec();
        h.resize(44, 0);
        h.extend(std::iter::repeat_n([0x00u8, 0x40], n).flatten());
        h
    };
    let mut b = head(3);
    b.extend(head(2));
    let parts = split_streamed(&b);
    assert_eq!(parts.iter().map(Vec::len).collect::<Vec<_>>(), vec![3, 2]);
    assert!((parts[0][0] - 0.5).abs() < 1e-6);
}

#[test]
fn silent_or_broken_readings_are_caught() {
    let voice: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
    assert!(heard_any(&[vec![0.0; 24_000], voice.clone()]));
    assert!(!heard_any(&[vec![0.0; 24_000], vec![]]));
    assert!(!heard_any(&[vec![0.0001; 24_000]]));
    let mut broken = voice;
    broken[100] = f32::NAN;
    assert!(!heard_any(&[broken]));
}

#[test]
fn takes_are_trimmed_checked_and_levelled() {
    let rate = REC_RATE as usize;
    let mut s = vec![0.0005f32; rate];
    s.extend((0..rate * 3).map(|i| 0.3 * (i as f32 * 0.05).sin()));
    s.extend(vec![0.0005f32; rate]);
    let (take, notes) = clean(&s, 2.0).unwrap();
    let secs = take.len() as f32 / REC_RATE as f32;
    assert!((3.0..3.7).contains(&secs), "{secs}");
    assert!(notes.is_empty(), "{notes:?}");
    assert!(clean(&s, 5.0).unwrap_err().contains("only"));
    assert!(clean(&vec![0.0; rate * 2], 1.0).is_err());
    // All voice from the first moment: kept whole, and not called noisy.
    let loud: Vec<f32> = (0..rate * 3).map(|i| if i % 50 < 5 { 1.0 } else { 0.4 * (i as f32 * 0.05).sin() }).collect();
    let (all, notes) = clean(&loud, 1.0).unwrap();
    assert_eq!(all.len(), loud.len());
    assert_eq!(notes, vec!["loud".to_string()]);
    // A hum under every pause is noise.
    let mut hum: Vec<f32> = (0..rate).map(|i| 0.02 * (i as f32 * 0.3).sin()).collect();
    hum.extend((0..rate * 2).map(|i| 0.1 * (i as f32 * 0.05).sin() + 0.02 * (i as f32 * 0.3).sin()));
    hum.extend((0..rate).map(|i| 0.02 * (i as f32 * 0.3).sin()));
    assert!(clean(&hum, 1.0).unwrap().1.contains(&"noisy".to_string()));
}

#[test]
fn voices_are_found_by_what_a_chat_calls_them() {
    let v = |id: &str, name: &str| OwnVoice { id: id.into(), name: name.into(), ..Default::default() };
    let voices = vec![v("a", "My voice"), v("b", "Grandpa Joe")];
    assert_eq!(find(&voices, "grandpa joe").map(|x| x.id.as_str()), Some("b"));
    assert_eq!(find(&voices, "Grandpa").map(|x| x.id.as_str()), Some("b"));
    assert_eq!(find(&voices, "me").map(|x| x.id.as_str()), Some("a"));
    assert!(find(&voices, "Mozart").is_none());
    assert!(find(&[], "my voice").is_none());
}

#[test]
fn voices_know_what_they_can_do() {
    let t = |l: &str| Take { line: l.into(), seconds: 3.0, ..Default::default() };
    let mut v = OwnVoice { takes: vec![t("intro"), t("story")], ..Default::default() };
    assert!(!v.can_speak() && !v.can_sing());
    v.takes.push(t("calm"));
    assert!(v.can_speak() && v.can_sing());
    let only_singing = OwnVoice { takes: vec![t("song")], ..Default::default() };
    assert!(!only_singing.can_speak() && only_singing.can_sing());
}

/// A song sung again in another voice, the way `revoice_item` does it,
/// written out for listening: SULCUSAI_SONG (default a Compose test
/// song), the voice from SULCUSAI_VOICE (default qwentts.cpp's example
/// speech), Seed-VC's pieces in SULCUSAI_SVC_DIR, the installed app's
/// separator and ONNX Runtime (the card's with SULCUSAI_ONNX_CARD).
/// Prints how much nearer the voice print came to the voice (CAMPPlus,
/// the converter's own, so listen too).
///     cargo test sings_a_song_in_a_recorded_voice -- --ignored --nocapture
#[test]
#[ignore]
fn sings_a_song_in_a_recorded_voice() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let song_path = PathBuf::from(env("SULCUSAI_SONG", r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3"));
    let voice_path = PathBuf::from(env("SULCUSAI_VOICE", r"C:\dev\tools\voice\qwentts.cpp\examples\freeman.wav"));
    let svc_dir = PathBuf::from(env("SULCUSAI_SVC_DIR", r"C:\dev\tools\voice\onnx16"));
    let out = PathBuf::from(env("SULCUSAI_OUT", r"C:\dev\tools\voice\tests\sung"));
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("app.sulcusai.desktop");
    let models = crate::storage::models_dir(&local);
    let card = std::env::var_os("SULCUSAI_ONNX_CARD").is_some();
    let rt = local.join("engines").join(if card { "onnxruntime-1.30.0-cuda13" } else { "onnxruntime-1.30.0-cpu-x64" });
    let dll = [rt.join("onnxruntime.dll"), rt.join("onnxruntime-win-x64-1.30.0").join("lib").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let card = card.then_some(crate::natural::Card { device: 0, dml: false });
    let decode = |p: &Path| {
        let (s, rate, ch) = super::super::audio_in::decode(p).unwrap();
        let ch = ch.max(1) as usize;
        stems::Stereo { rate, ch: [s.chunks(ch).map(|f| f[0]).collect(), s.chunks(ch).map(|f| f[if ch > 1 { 1 } else { 0 }]).collect()] }.resampled(stems::RATE)
    };
    let started = std::time::Instant::now();
    let song = decode(&song_path);
    let reference = decode(&voice_path).mono();
    let (parts, _) = stems::separate(&dll, &models.join("media").join("helpers").join("htdemucs_fp16weights.onnx"), &song, 8, &[0, 1, 2, 3], card, &AtomicBool::new(false)).unwrap();
    stems::unload();
    let apart = started.elapsed().as_secs_f64();
    let band = stems::Stereo { rate: stems::RATE, ch: [0, 1].map(|c| (0..song.len()).map(|i| parts[0].ch[c][i] + parts[1].ch[c][i] + parts[2].ch[c][i]).collect()) };
    let sung = parts[3].mono();
    let mut conv = svc::Converter::new(&dll, &svc_dir, 8, card).unwrap();
    let v = conv.voice(&reference).unwrap();
    let song_pitch = conv.middle_pitch_of(&sung).unwrap();
    let semitones = svc::octave_shift(song_pitch, v.pitch * 1.26);
    let steps = if card.is_some() { 30 } else { 10 };
    let t = std::time::Instant::now();
    let never = AtomicBool::new(false);
    let new_voice = conv.convert(&v, &sung, svc::Settings { steps, semitones, ..Default::default() }, &never, &|_| {}).unwrap();
    println!(
        "{:.0} s song: apart in {apart:.0} s, sung again in {:.0} s ({} steps, {semitones} semitones: song {song_pitch:.0} Hz, voice {:.0} Hz) on the {}",
        song.len() as f64 / stems::RATE as f64,
        t.elapsed().as_secs_f64(),
        steps,
        v.pitch,
        if conv.on_card { "card" } else { "processor" }
    );
    let mixed = mix(&band, &sung, &new_voice);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("as-made.wav"), wav_stereo(&song.ch[0], &song.ch[1], stems::RATE)).unwrap();
    std::fs::write(out.join("in-the-voice.wav"), wav_stereo(&mixed.ch[0], &mixed.ch[1], stems::RATE)).unwrap();
    std::fs::write(out.join("voice-alone.wav"), crate::audio::wav_encode(&new_voice, stems::RATE)).unwrap();
    // Voice prints of the voice, the song's singer and the new singer.
    let mut print = |x: &[f32]| {
        let w16 = stems::resample(x, stems::RATE, 16_000);
        let (n, fb) = crate::diarize::fbank_with(&w16, true);
        let e = conv.run("campplus.onnx", vec![("fbank", vec![1, n as i64, 80], fb)]).unwrap();
        let norm = e.iter().map(|v| v * v).sum::<f32>().sqrt();
        e.into_iter().map(|v| v / norm).collect::<Vec<f32>>()
    };
    let (target, before, after) = (print(&reference), print(&sung), print(&new_voice));
    let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    println!("voice print likeness to the voice: song's singer {:.2}, sung again {:.2}", cos(&before, &target), cos(&after, &target));
    assert!(cos(&after, &target) > cos(&before, &target));
}

/// Narration in a recorded voice through the engine, as `speak` runs it:
/// qwentts.cpp in SULCUSAI_TTS_DIR (default the graphics-card build),
/// its models in SULCUSAI_TTS_MODELS, the voice from qwentts.cpp's
/// example speech and its words. Prints the time and how near the voice
/// print came (CAMPPlus, from Seed-VC's pieces in SULCUSAI_SVC_DIR);
/// writes the reading out for listening.
///     cargo test reads_in_a_recorded_voice -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn reads_in_a_recorded_voice() {
    let env = |k: &str, d: &str| PathBuf::from(std::env::var(k).unwrap_or_else(|_| d.to_string()));
    let dir = env("SULCUSAI_TTS_DIR", r"C:\dev\qb");
    let models = env("SULCUSAI_TTS_MODELS", r"C:\dev\tools\voice\models");
    let example = PathBuf::from(r"C:\dev\tools\voice\qwentts.cpp\examples");
    let big = std::env::var_os("SULCUSAI_TTS_SMALL").is_none();
    let files = [models.join(if big { "qwen-talker-1.7b-base-Q8_0.gguf" } else { "qwen-talker-0.6b-base-Q8_0.gguf" }), models.join("qwen-tokenizer-12hz-Q8_0.gguf")];
    let pcm = crate::audio::wav_decode(&std::fs::read(example.join("freeman.wav")).unwrap()).unwrap();
    let reference = stems::resample(&pcm.samples, pcm.rate, TTS_RATE);
    let words = std::fs::read_to_string(example.join("freeman.txt")).unwrap();
    let lines = lines_of("Your package from the bakery arrives on Thursday afternoon. Please leave the side gate open, and the dog inside!");
    let t = std::time::Instant::now();
    let never = AtomicBool::new(false);
    let (said, rate) = read_with(&dir.join(TTS_EXE), &files, &reference, words.trim(), &lines, Some("en"), None, &never).await.unwrap();
    let secs = said.len() as f64 / rate as f64;
    println!("{} lines, {secs:.1} s of speech in {:.1} s ({})", lines.len(), t.elapsed().as_secs_f64(), if big { "1.7B" } else { "0.6B" });
    assert!(secs > 3.0);
    let out = env("SULCUSAI_OUT", r"C:\dev\tools\voice\tests");
    std::fs::write(out.join("read-in-the-voice.wav"), crate::audio::wav_encode(&said, rate)).unwrap();
    // Voice prints: the reading against the voice, and against a voice
    // of Windows' own reading the same words.
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("app.sulcusai.desktop");
    let rt = local.join("engines").join("onnxruntime-1.30.0-cpu-x64");
    let dll = [rt.join("onnxruntime.dll"), rt.join("onnxruntime-win-x64-1.30.0").join("lib").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
    let mut conv = svc::Converter::new(&dll, &env("SULCUSAI_SVC_DIR", r"C:\dev\tools\voice\onnx16"), 8, None).unwrap();
    let mut print = |x: &[f32], from: u32| {
        let w16 = stems::resample(x, from, 16_000);
        let (n, fb) = crate::diarize::fbank_with(&w16, true);
        let e = conv.run("campplus.onnx", vec![("fbank", vec![1, n as i64, 80], fb)]).unwrap();
        let norm = e.iter().map(|v| v * v).sum::<f32>().sqrt();
        e.into_iter().map(|v| v / norm).collect::<Vec<f32>>()
    };
    let other = crate::tts::synthesize(&lines.join(" "), None, Some("en"), 1.0).unwrap();
    let other_samples: Vec<f32> = other.samples.iter().map(|s| *s as f32 / 32768.0).collect();
    let (target, clone, windows) = (print(&reference, TTS_RATE), print(&said, rate), print(&other_samples, other.rate));
    let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    println!("voice print likeness to the voice: the reading {:.2}, Windows' voice {:.2}", cos(&clone, &target), cos(&windows, &target));
    assert!(cos(&clone, &target) > cos(&windows, &target));
}

/// The installed app's recorded voices read aloud the way Your voice's
/// "Read it in this voice" does, from its own data (read only: the
/// database opened read-only, takes decrypted in memory), its engine and
/// models. Prints each voice's takes (lines, lengths, notes; not the
/// words), the reference and the reading or the error; writes the
/// reading to SULCUSAI_OUT for listening.
///     cargo test real_own_voice_reads -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn real_own_voice_reads() {
    let data = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("app.sulcusai.desktop");
    let models = crate::storage::models_dir(&data);
    let paths = Paths { models: models.clone(), engines: data.join("engines"), logs: data.join("logs"), db: data.join("sulcusai.db"), data: data.clone() };
    let conn = Connection::open_with_flags(&paths.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let vault = crate::crypto::Vault::open(&data.join("keys.json"), crate::crypto::platform_protector(&data)).unwrap();
    let c = vault.work_cipher().expect("the app's data key is locked with a password");
    let voices = list(&conn, &c);
    println!("{} voice(s)", voices.len());
    for v in &voices {
        println!("voice \"{}\": can speak {}, can sing {}", v.name, v.can_speak(), v.can_sing());
        for t in &v.takes {
            println!("  {}: {:.1} s, {} words, notes {:?}", t.line, t.seconds, t.words.split_whitespace().count(), t.notes);
        }
        let (reference, words) = match speech_reference(&paths, &c, v) {
            Ok(r) => r,
            Err(e) => {
                println!("  reference: {e}");
                continue;
            }
        };
        println!("  reference: {:.1} s, {} words", reference.len() as f32 / TTS_RATE as f32, words.split_whitespace().count());
        let exe = paths.engines.join("qwentts-5916bf7-vulkan-x64").join(TTS_EXE);
        let files = [models.join("media").join("own-voice").join("qwen-talker-1.7b-base-Q8_0.gguf"), models.join("media").join("own-voice").join("qwen-tokenizer-12hz-Q8_0.gguf")];
        let lines = lines_of("This is how I sound when SulcusAI reads something for me.");
        let never = AtomicBool::new(false);
        let t = std::time::Instant::now();
        // SULCUSAI_TTS_DEVICE=CPU: on the processor, as when the card is full.
        let device = std::env::var("SULCUSAI_TTS_DEVICE").ok();
        match read_with(&exe, &files, &reference, &words, &lines, Some("en"), device.as_deref(), &never).await {
            Ok((said, rate)) => {
                println!("  read: {:.1} s of speech in {:.1} s", said.len() as f32 / rate as f32, t.elapsed().as_secs_f64());
                let out = PathBuf::from(std::env::var("SULCUSAI_OUT").unwrap_or_else(|_| r"C:\dev\tools\voice\tests".into()));
                std::fs::write(out.join(format!("real-{}.wav", v.id)), crate::audio::wav_encode(&said, rate)).unwrap();
            }
            Err(e) => println!("  read failed: {e}"),
        }
    }
}

/// Recordings added to a voice, as Your voice's "Add recordings" does: a
/// song (its voice taken out of the music) as singing and the example
/// speech as speaking, in a throwaway app data folder (the separator
/// and its runtime from the real ones). SULCUSAI_SONG picks the song.
///     cargo test songs_and_speech_are_added -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn songs_and_speech_are_added_to_a_voice() {
    let state = crate::testutil::temp_state(None);
    let c = state.work_cipher().unwrap();
    let id = add_for_test(&state, "Added", &[]);
    let quiet = |_: &str| {};
    let song = PathBuf::from(std::env::var("SULCUSAI_SONG").unwrap_or_else(|_| r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3".into()));
    let speech = PathBuf::from(r"C:\dev\tools\voice\qwentts.cpp\examples\freeman.wav");
    let t = std::time::Instant::now();
    add_one(&state, &c, &id, &song, LineKind::Sing, &quiet).await.unwrap();
    println!("song added in {:.0} s", t.elapsed().as_secs_f64());
    add_one(&state, &c, &id, &speech, LineKind::Speak, &quiet).await.unwrap();
    // Too short to use.
    let short = std::env::temp_dir().join(format!("sulcusai-short-{}.wav", super::super::store::new_id()));
    std::fs::write(&short, crate::audio::wav_encode(&vec![0.1f32; 32_000], 16_000)).unwrap();
    assert!(add_one(&state, &c, &id, &short, LineKind::Sing, &quiet).await.unwrap_err().contains("shorter"));
    let _ = std::fs::remove_file(&short);
    let v = get(&state.db.lock().unwrap(), &c, &id).unwrap();
    for t in &v.takes {
        println!("{} ({:?}, {}): {:.1} s, voice {:.0}%, notes {:?}, {} words", t.line, t.kind, t.file, t.seconds, t.score * 100.0, t.notes, t.words.split_whitespace().count());
    }
    assert_eq!(v.takes.len(), 2);
    assert!(v.can_sing() && v.can_speak(), "an added song sings and 15 s of speech reads");
    let (sing_ref, sung) = singing_reference(&state.paths, &c, &v).unwrap();
    assert!(sung && sing_ref.len() as f32 / svc::RATE as f32 > 8.0);
    let (speak_ref, _) = speech_reference(&state.paths, &c, &v).unwrap();
    assert!(speak_ref.len() as f32 / TTS_RATE as f32 > 8.0);
}

#[test]
fn the_clearest_stretch_is_where_the_voice_is() {
    let rate = 1000;
    // 20 s quiet, 10 s of voice, 20 s quiet: a 6 s stretch lands inside the voice.
    let mut s = vec![0.0005f32; rate * 20];
    s.extend((0..rate * 10).map(|i| 0.3 * (i as f32 * 0.7).sin()));
    s.extend(vec![0.0005f32; rate * 20]);
    let (a, b, score) = clearest(&s, rate as u32, 6.0);
    assert!(a >= rate * 20 && b <= rate * 30, "{a}..{b}");
    assert_eq!(b - a, rate * 6);
    assert!(score > 0.95, "{score}");
    // Shorter than asked: all of it.
    let (a, b, _) = clearest(&s[..rate * 3], rate as u32, 6.0);
    assert_eq!((a, b), (0, rate * 3));
}

#[test]
fn voices_saved_before_added_recordings_read_the_same() {
    // A voice as 0.9.34 saved it: no kind, file or score on its takes.
    let old = r#"[{"id":"v1","name":"Ike","takes":[{"line":"intro","seconds":6.5,"words":"Hi","notes":[],"at":1},{"line":"story","seconds":6.9,"words":"The","notes":[],"at":2},{"line":"calm","seconds":6.3,"words":"Take","notes":[],"at":3},{"line":"song","seconds":24.1,"words":"","notes":[],"at":4}],"created_at":1,"agreed_at":1}]"#;
    let voices: Vec<OwnVoice> = serde_json::from_str(old).unwrap();
    let v = &voices[0];
    assert!(v.can_speak() && v.can_sing());
    assert_eq!(v.order(LineKind::Sing, SING_ORDER)[0], "song");
    assert_eq!(v.order(LineKind::Speak, SPEAK_ORDER)[0], "everyday");
    // Added recordings go first, the clearest before the rest; the script's takes stay.
    let mut v = v.clone();
    let added = |line: &str, score: f32, kind: LineKind, seconds: f32| Take { line: line.into(), kind: Some(kind), score, seconds, ..Default::default() };
    v.takes.push(added("added-1", 0.6, LineKind::Sing, 15.0));
    v.takes.push(added("added-2", 0.9, LineKind::Sing, 15.0));
    assert_eq!(&v.order(LineKind::Sing, SING_ORDER)[..3], ["added-2", "added-1", "song"]);
    assert_eq!(v.takes.len(), 6);
    // One added reading of 8 s is enough to narrate.
    let only = OwnVoice { takes: vec![added("added-1", 0.8, LineKind::Speak, 9.0)], ..Default::default() };
    assert!(only.can_speak());
}

#[test]
fn a_line_may_take_about_its_length() {
    // 72 characters: 12 s at most, while a natural reading is ~5 s.
    let line = "The bakery opens at seven. Bring the blue umbrella, it might rain later.";
    assert!((longest_s(line) - 12.0).abs() < 0.1, "{}", longest_s(line));
    assert!((longest_s("Hi.") - 3.375).abs() < 0.01);
}

#[test]
fn languages_get_the_engines_names() {
    assert_eq!(tts_language(Some("en-US")), "English");
    assert_eq!(tts_language(Some("pt")), "Portuguese");
    assert_eq!(tts_language(Some("nl")), "auto");
    assert_eq!(tts_language(None), "auto");
}

#[test]
fn the_new_voice_sits_where_the_old_one_was() {
    let band = stems::Stereo { rate: 44_100, ch: [vec![0.1; 44_100], vec![0.1; 44_100]] };
    let old: Vec<f32> = (0..44_100).map(|i| 0.2 * (i as f32 * 0.03).sin()).collect();
    let new: Vec<f32> = old.iter().map(|x| x * 0.25).collect();
    let m = mix(&band, &old, &new);
    let voice: Vec<f32> = m.ch[0].iter().map(|x| x - 0.1).collect();
    let r = |x: &[f32]| crate::audio::rms(x);
    assert!((r(&voice) / r(&old) - 1.0).abs() < 0.05, "{}", r(&voice) / r(&old));
}

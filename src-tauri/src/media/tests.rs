// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

/// Made real, a composition keeps every song option picked (emotion,
/// ways of singing, harmonies…) and the words given, with its own tempo,
/// key and time signature; without options, the ones it was written with.
#[test]
fn a_composition_is_made_real_as_asked() {
    let plan = compose::Plan {
        bpm: 92,
        beats: 6,
        unit: 8,
        key: "D major".into(),
        style: "folk".into(),
        voice: "male".into(),
        sections: vec![compose::PlanSection { name: "Verse".into(), lines: vec!["Old words".into()], ..Default::default() }],
        song: Some(MusicSpec { emotion: Some("Hopeful".into()), bpm: Some(140), ..Default::default() }),
        ..Default::default()
    };
    let asked = MusicSpec { emotion: Some("Heartbroken, Longing".into()), vocal_style: Some("raspy".into()), harmony_voice: Some("female".into()), sections: vec!["Intro".into()], ..Default::default() };
    let r = real_request(&plan, "demo", None, None, None, Some(asked), Some("[Verse]\nNew words".into()), Some("en".into()));
    let m = r.music.unwrap();
    assert_eq!((m.emotion.as_deref(), m.vocal_style.as_deref(), m.harmony_voice.as_deref()), (Some("Heartbroken, Longing"), Some("raspy"), Some("female")));
    assert_eq!((m.bpm, m.key.as_deref(), m.time_signature.as_deref(), m.task.as_deref(), m.audio.as_deref()), (Some(92), Some("D major"), Some("6/8"), Some("cover"), Some("demo")));
    assert_eq!((m.genre.as_deref(), m.voice.as_deref(), m.strength), (Some("folk"), Some("male"), Some(REAL_STRENGTH_SUNG)));
    assert!(m.sections.is_empty(), "the composition's own shape");
    assert_eq!((r.lyrics.as_deref(), r.language.as_deref()), (Some("[Verse]\nNew words"), Some("en")));
    // Nothing asked: what it was written with, and its own words.
    let r = real_request(&plan, "demo", None, None, None, None, None, None);
    let m = r.music.as_ref().unwrap();
    assert_eq!((m.emotion.as_deref(), m.bpm), (Some("Hopeful"), Some(92)));
    assert_eq!(r.lyrics.as_deref(), Some("[Verse]\nOld words"));
}

/// Making a song's unclear parts again is a music model's job (no music
/// model says it can "remake": the job failed for want of one).
#[test]
fn a_remake_is_made_by_a_music_model() {
    let remake = Request { op: "remake".into(), ..Default::default() };
    assert_eq!((model_op(&remake), op_kind(model_op(&remake))), ("music", Some(Kind::Music)));
    let parts = Request { op: "music".into(), music: Some(MusicSpec { task: Some("extract".into()), ..Default::default() }), ..Default::default() };
    assert_eq!(model_op(&parts), "parts");
}

/// Harmonies, the lead's style and the arrangement become the music
/// model's words; the lyrics writer is told the sections and the echoes.
#[test]
fn harmonies_and_the_arrangement_are_described() {
    let spec = MusicSpec {
        genre: Some("Pop".into()),
        voice: Some("female".into()),
        vocal_style: Some("Breathy".into()),
        harmony_voice: Some("male".into()),
        harmony_style: Some("thirds".into()),
        harmony_instrument: Some("Strings".into()),
        lead_instrument: Some("Electric guitar".into()),
        energy: Some("big-chorus".into()),
        production: Some("1980s".into()),
        ending: Some("fade".into()),
        structure: Some("verse-chorus-bridge".into()),
        ..Default::default()
    };
    assert_eq!(
        spec.caption("summer love", false),
        "summer love, breathy female lead vocals, pop, electric guitar lead melody and solo, male backing harmony vocals in close harmony a third above the lead on the choruses, \
         strings playing a harmony line under the melody on the choruses, soft verses and a big, powerful chorus, 1980s production, fades out at the end, sung by a woman, female voice throughout"
    );
    assert!(!spec.caption("", true).contains("harmony vocals"), "an instrumental has no sung harmonies");
    assert!(spec.caption("", true).contains("strings playing a harmony line"), "but keeps the instrument's");
    // A decade: the song's style and sound, and how its words are written.
    let old = MusicSpec { genre: Some("Rock".into()), era: Some("1970s".into()), ..Default::default() };
    assert_eq!(old.caption("road trip", true), "road trip, rock, 1970s style, a song from the 1970s with the instruments and recording sound of that era, instrumental, no vocals");
    assert!(old.lyrics_guide().contains("a song from the 1970s"));
    let mixed = MusicSpec { harmony_voice: Some("mixed".into()), harmony_where: Some("ending".into()), harmony_style: Some("oohs".into()), ..Default::default() };
    assert_eq!(mixed.harmony_vocals().unwrap(), "male and female backing harmony vocals singing soft oohs and aahs on the final chorus");
    let guide = spec.lyrics_guide();
    assert_eq!(spec.sections(), ["Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus"]);
    assert!(guide.contains("round brackets") && guide.contains("chorus lines"), "{guide}");
    assert_eq!(MusicSpec::default().lyrics_guide(), "");
    // Sections picked one by one (choruses repeated) and a rhyme pattern.
    let mine = MusicSpec {
        sections: ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Solo", "Chorus", "Outro"].map(String::from).to_vec(),
        structure: Some("aaba".into()),
        rhyme: Some("abab".into()),
        ..Default::default()
    };
    assert_eq!(mine.sections(), ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Solo", "Chorus", "Outro"], "the chosen sections win");
    let g = mine.lyrics_guide();
    assert!(g.contains("(ABAB)"), "{g}");
    assert!(mine.caption("", true).contains("arranged with chorus, intro, outro, solo, verse"));
    for (i, p) in [("Strings", "strings"), ("Grand piano", "keyboard"), ("Synth pad", "synth"), ("Trumpet", "brass"), ("Flute", "woodwinds"), ("Acoustic guitar", "guitar")] {
        assert_eq!(part_for(i), Some(p), "{i}");
    }
    assert_eq!(part_for("Kazoo"), None);
}

/// Auto length: a length in the description, else the person's lyrics,
/// else the sections, else (written lyrics) none yet.
#[test]
fn auto_length_follows_the_song() {
    let auto = |prompt: &str, lyrics: Option<&str>, sections: &[&str], instrumental: bool| {
        let music = MusicSpec { auto_length: true, sections: sections.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        auto_seconds(&Request { op: "music".into(), prompt: prompt.into(), lyrics: lyrics.map(str::to_string), instrumental: Some(instrumental), music: Some(music), ..Default::default() })
    };
    assert_eq!(auto("a 90-second jingle for a bakery", None, &[], false), Some(90.0));
    assert_eq!(auto("a little jingle for a bakery", None, &[], false), Some(30.0), "a jingle is short");
    assert_eq!(auto("an epic ballad, about 4 minutes long", None, &["Verse"], false), Some(240.0), "named beats the sections");
    let mine = "[Verse]\na b c d\ne f g h\ni j k l\nm n o p\n\n[Chorus]\nq r s t\nu v w x\ny z a b\nc d e f";
    assert_eq!(auto("a love song", Some(mine), &[], false), Some(65.0), "8 lines of 7.5 s and a lead-in");
    assert_eq!(auto("a love song", None, &["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"], false), Some(155.0));
    assert_eq!(auto("chill beats", None, &[], true), Some(90.0), "an instrumental");
    assert_eq!(auto("a love song", None, &[], false), None, "written lyrics decide it");
    // Not Auto, or a length given: left alone.
    let fixed = Request { op: "music".into(), prompt: "a 90-second jingle".into(), seconds: Some(30.0), music: Some(MusicSpec { auto_length: true, ..Default::default() }), ..Default::default() };
    assert_eq!(auto_seconds(&fixed), None);
    let off = Request { op: "music".into(), prompt: "a 90-second jingle".into(), music: Some(MusicSpec::default()), ..Default::default() };
    assert_eq!(auto_seconds(&off), None);
}

/// A song in a recorded voice gets its singer's sex from the recording
/// only when the description doesn't already say who sings.
#[test]
fn a_described_singer_is_kept() {
    assert!(names_a_singer("A country song with a female singer"));
    assert!(names_a_singer("boy band pop, 2000s"));
    assert!(names_a_singer("Duet about the sea"));
    assert!(!names_a_singer("heavy bass, human connection, a manic beat"));
    assert!(!names_a_singer("a love song for my wife"));
}

/// A feeling, and ways of singing on parts of the song: in the
/// description, on the sections' tags (one word each), and for the
/// lyrics writer.
#[test]
fn emotion_and_vocal_effects_are_sung() {
    let fx = |how: &str, on: &str| VocalEffect { how: how.into(), on: on.into() };
    let spec = MusicSpec {
        voice: Some("male".into()),
        emotion: Some("Heartbroken".into()),
        vocal_effects: vec![fx("vibrato", "all"), fx("screamed", "chorus"), fx("whispered", "verse"), fx("belting", "final-chorus")],
        ..Default::default()
    };
    assert_eq!(
        spec.caption("losing you", false),
        "losing you, male lead vocals, heartbroken, emotional vocal delivery, vocals with heavy vibrato throughout, screamed vocals, screamo on the choruses, \
         whispered vocals on the verses, powerful belted vocals on the final chorus, sung by a man, male voice throughout"
    );
    assert_eq!(spec.caption("", true), "heartbroken feeling, instrumental, no vocals", "an instrumental has a feeling but no singing");
    let lyrics = "[Verse]\na\n\n[Chorus]\nb\n\n[Verse 2]\nc\n\n[Chorus]\nb\n\n[Chorus - softer]\nd\n\n[Chorus]\nb";
    assert_eq!(
        spec.mark_sections(lyrics),
        "[Verse - whispered]\na\n\n[Chorus - screamed]\nb\n\n[Verse 2 - whispered]\nc\n\n[Chorus - screamed]\nb\n\n[Chorus - softer]\nd\n\n[Chorus - belting]\nb",
        "the whole song's vibrato is only in the description; a tag that says something stays"
    );
    assert_eq!(MusicSpec::default().mark_sections(lyrics), lyrics);
    let g = spec.lyrics_guide();
    assert!(g.contains("feeling: heartbroken") && g.contains("under [Chorus] tags are screamed") && g.contains("under [Verse] tags are whispered"), "{g}");
    let custom = MusicSpec { vocal_effects: vec![fx("Yodelling", "bridge")], ..Default::default() };
    assert_eq!(custom.caption("", false), "yodelling vocals on the bridge");
    assert_eq!(custom.mark_sections("[Bridge]\nla"), "[Bridge - yodelling]\nla");
}

/// Suno's advanced options: styles left out of the description and the
/// words, Weirdness at either end, a title the hook is built around, and
/// Style influence as the music model's guidance.
#[test]
fn styles_are_left_out_and_weirdness_is_said() {
    let spec = MusicSpec {
        genre: Some("Rock".into()),
        instruments: vec!["electric guitar".into(), "drums".into(), "drumsticks clicking".into()],
        production: Some("lo-fi".into()),
        exclude: Some("no drums, Lo-Fi; autotune".into()),
        weirdness: Some(0.9),
        title: Some("Paper Moon".into()),
        ..Default::default()
    };
    assert_eq!(spec.excluded(), vec!["drums", "lo-fi", "autotune"]);
    assert_eq!(
        spec.caption("drums and bass night drive", true),
        "drums and bass night drive, rock, electric guitar, drumsticks clicking, instrumental, no vocals, experimental, unexpected arrangement, unusual sounds and surprising turns",
        "the person's own words stay; a part naming what's left out goes"
    );
    let g = spec.lyrics_guide();
    assert!(g.contains("called \"Paper Moon\"") && g.contains("Leave these out of the words: drums, lo-fi, autotune") && g.contains("Be daring"), "{g}");
    let plain = MusicSpec { weirdness: Some(0.1), ..Default::default() };
    assert_eq!(plain.caption("", true), "instrumental, no vocals, classic, familiar arrangement");
    assert_eq!(music::guidance("acestep-v15-xl-turbo-Q8_0.gguf", None), None);
    assert_eq!(music::guidance("acestep-v15-xl-turbo-Q8_0.gguf", Some(0.5)), Some(3.0));
    assert_eq!(music::guidance("acestep-v15-base-Q8_0.gguf", None), Some(music::FULL_GUIDANCE));
    assert_eq!(music::guidance("acestep-v15-xl-sft-Q8_0.gguf", Some(1.0)), Some(11.0));
}

/// The Studio's choices become the music model's description; a part
/// job asks for the model that does parts; lyrics are written only for
/// songs that sing.
#[test]
fn a_shaped_song_is_described_and_routed() {
    let spec = MusicSpec { genre: Some("Lo-fi hip hop".into()), mood: Some("Calm".into()), instruments: vec!["Piano".into(), "Vinyl crackle".into()], voice: Some("female".into()), beat: Some("Boom bap".into()), ..Default::default() };
    assert_eq!(spec.caption("rainy evening study session.", false), "rainy evening study session, female lead vocals, lo-fi hip hop, calm mood, piano, vinyl crackle, boom bap beat, sung by a woman, female voice throughout");
    assert_eq!(spec.caption("", true), "lo-fi hip hop, calm mood, piano, vinyl crackle, instrumental, no vocals, boom bap beat");
    assert!(spec.describes() && !MusicSpec::default().describes());
    let part = |task: &str, track: &str| Request { op: "music".into(), music: Some(MusicSpec { task: Some(task.into()), track: Some(track.into()), audio: Some("a".into()), ..Default::default() }), ..Default::default() };
    assert_eq!(model_op(&part("add", "drums")), "parts");
    assert_eq!(model_op(&part("cover", "")), "music");
    assert_eq!(model_op(&Request { op: "sound".into(), ..Default::default() }), "sound");
    // Lyrics are written for a new song with vocals, and for vocals added; not for a cover or drums.
    assert!(wants_written_lyrics(&Request { op: "music".into(), ..Default::default() }));
    assert!(wants_written_lyrics(&part("add", "vocals")));
    assert!(!wants_written_lyrics(&part("add", "drums")) && !wants_written_lyrics(&part("cover", "")));
    assert!(!wants_written_lyrics(&Request { op: "music".into(), instrumental: Some(true), ..Default::default() }));
}

#[test]
fn captions_are_lines_a_viewer_can_read() {
    use crate::speech::Segment;
    let long = "word ".repeat(40);
    let lines = caption_lines(&[Segment { start: 0.0, end: 2.0, text: " Hello there. ".into() }, Segment { start: 2.0, end: 16.0, text: long.clone() }]);
    assert_eq!(lines[0], (0.0, 2.0, "Hello there.".to_string()));
    assert!(lines.len() >= 4 && lines[1..].iter().all(|(a, b, t)| b - a <= 7.01 && t.chars().count() <= 84), "{lines:?}");
    assert!((lines.last().unwrap().1 - 16.0).abs() < 1e-9, "the last ends with the stretch");
    assert_eq!(srt(&lines[..1]), "1\n00:00:00,000 --> 00:00:02,000\nHello there.\n\n");
    assert_eq!(srt_time(3725.5), "01:02:05,500");
    assert_eq!(filter_path(std::path::Path::new(r"C:\work\it's.srt")), r"C\:/work/it\'s.srt");
}

#[test]
fn pictures_look_real_unless_another_look_is_asked_for() {
    assert!(styled("A red fox in a snowy forest", None).contains("realistic photograph"));
    assert!(styled("A red fox in a snowy forest.", Some("auto")).starts_with("A red fox in a snowy forest, realistic"));
    assert_eq!(styled("A red fox in a snowy forest, watercolor style", None), "A red fox in a snowy forest, watercolor style");
    assert!(styled("A cute robot", Some("cartoon")).contains("cartoon illustration"));
    assert!(styled("A castle", Some("oil_painting")).contains("oil painting"));
    assert_eq!(styled("A castle", Some("nonsense")), "A castle");
    // Whole words only: an iconic photographer still gets the photo look.
    assert!(styled("An iconic photographer at work", None).contains("realistic photograph"));
    assert!(has_word("two 3d icons", "icon") && !has_word("iconic", "icon"));
    // A cartoon keeps its look: the photo-only negative words go.
    let sdxl = "(worst quality, low quality, illustration, 3d, 2d, painting, cartoons, sketch), open mouth, deformed";
    assert_eq!(fit_negative(sdxl, Some("cartoon"), "a cute robot"), "worst quality, low quality, open mouth, deformed");
    assert_eq!(fit_negative(sdxl, Some("as_written"), "a cute robot, watercolor painting, soft washes"), "worst quality, low quality, open mouth, deformed");
    assert_eq!(fit_negative(sdxl, Some("photo"), "a red fox"), sdxl, "photos keep it whole");
    assert!(!card_trouble("failed to load model"), "a broken file isn't the card's fault");
    assert!(card_trouble("ggml_vulkan: Device memory allocation failed: ErrorOutOfDeviceMemory"));
    assert!(card_trouble("diffusion model compute failed"));
    let args: Vec<std::ffi::OsString> = ["-M", "img_gen", "--max-vram", "3.0", "-o", "x.png"].iter().map(Into::into).collect();
    assert_eq!(without_vram(&args), ["-M", "img_gen", "-o", "x.png"].iter().map(std::ffi::OsString::from).collect::<Vec<_>>());
    // Planned for an 8 GB card: 1 GiB kept free and tiled decoding; after
    // a failure, more free, the text reader on the processor, no fast attention.
    let job: Vec<std::ffi::OsString> = ["-M", "img_gen", "--diffusion-fa", "-W", "1024", "-H", "1024", "-o", "x.png"].iter().map(Into::into).collect();
    let first = card_friendly(&job, 8.0);
    let s = |v: &[std::ffi::OsString]| v.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
    assert_eq!(s(&first), "-M img_gen --diffusion-fa -W 1024 -H 1024 --max-vram -1 --vae-tiling -o x.png");
    assert_eq!(s(&lighter_on_card(&first, true)), "-M img_gen -W 1024 -H 1024 --max-vram -3 --vae-tiling --backend diffusion=Vulkan0,te=cpu,vae=Vulkan0 -o x.png");
    assert!(!s(&lighter_on_card(&first, false)).contains("--backend"), "not knowing which card is which, nothing is moved");
    let capped: Vec<std::ffi::OsString> = ["-M", "img_gen", "--max-vram", "6.0", "-o", "x.png"].iter().map(Into::into).collect();
    assert!(s(&lighter_on_card(&capped, true)).contains("--max-vram 3.0"), "a set budget is halved");
    assert!(!s(&card_friendly(&job, 16.0)).contains("--vae-tiling"), "a big card decodes in one go");
    // Video on a 12 GB card: the weights wait in memory, so decoding has room.
    let clip: Vec<std::ffi::OsString> = ["-M", "vid_gen", "-W", "832", "-H", "480", "-o", "x.webm"].iter().map(Into::into).collect();
    assert!(s(&card_friendly(&clip, 11.7)).contains("--offload-to-cpu"));
    assert!(!s(&card_friendly(&clip, 24.0)).contains("--offload-to-cpu"), "a big card keeps them");
    assert!(!s(&first).contains("--offload-to-cpu"), "pictures don't need it");
    // A clip moved by a voice (S2V) streams its weights, with no cap on top.
    let talk: Vec<std::ffi::OsString> = ["-M", "vid_gen", "--audio", "v.wav", "-W", "640", "-H", "368", "-o", "x.png"].iter().map(Into::into).collect();
    let t = card_friendly(&talk, 11.7);
    assert!(s(&t).contains("--offload-to-cpu") && !s(&t).contains("--max-vram"), "{}", s(&t));
    assert!(!s(&lighter_on_card(&t, true)).contains("--max-vram"));
    // On the processor, a 1184x880 picture becomes 640x480 (the longer edge 640).
    let big: Vec<std::ffi::OsString> = ["-W", "1184", "-H", "880", "-o", "x.png"].iter().map(Into::into).collect();
    assert_eq!(smaller(&big, 640), ["-W", "640", "-H", "480", "-o", "x.png"].iter().map(std::ffi::OsString::from).collect::<Vec<_>>());
}

fn hw(vram_gb: u64, cores: usize) -> (Hardware, Budget) {
    let mut h = Hardware { cpu_cores: Some(cores), cpu_threads: cores * 2, avx2: true, ram_total: 32 * GIB, disk_free: 500 * GIB, ..Default::default() };
    if vram_gb > 0 {
        h.gpus = vec![crate::hardware::Gpu { name: "GPU".into(), vram_bytes: vram_gb * GIB, ..Default::default() }];
    }
    let b = Budget::from_hardware(&h);
    (h, b)
}

fn model(id: &str) -> MediaModel {
    crate::catalog::Catalog::bundled().media.models.into_iter().find(|m| m.id == id).unwrap()
}

/// The Studio page's view returns, unlocked and locked (it once took the
/// database lock twice in one expression and froze the app).
#[cfg(windows)]
#[test]
fn studio_view_does_not_deadlock() {
    let state = crate::testutil::temp_state(None);
    for _ in 0..2 {
        let s = state.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(view(&s, false).gallery).ok());
        assert!(rx.recv_timeout(std::time::Duration::from_secs(30)).is_ok(), "media view never returned");
        state.vault.lock().unwrap().lock();
    }
}

#[test]
fn video_needs_a_graphics_card_and_says_why() {
    let (h, b) = hw(0, 8);
    let f = fit(&model("wan2.2-ti2v-5b"), &h, &b, 0, None);
    assert!(!f.runnable);
    assert!(f.reason.unwrap().contains("this PC has none"));
    let (h, b) = hw(12, 8);
    assert!(fit(&model("wan2.2-ti2v-5b"), &h, &b, 0, None).on_gpu);
}

#[test]
fn what_making_it_took_is_described_plainly() {
    assert_eq!(approx_tokens("a man playing frisbee with a dog in a park"), 13);
    assert_eq!(approx_tokens(""), 1);
    assert_eq!(cloud_label("cloud:openai:gpt-image-1"), "gpt-image-1 (openai)");
    assert_eq!(cloud_label("odd"), "odd");
}

#[test]
fn pictures_run_anywhere_but_slower_without_a_card() {
    let (h0, b0) = hw(0, 8);
    let (h1, b1) = hw(12, 8);
    let m = model("flux2-klein-4b");
    let cpu = fit(&m, &h0, &b0, 0, None);
    let gpu = fit(&m, &h1, &b1, 0, None);
    assert!(cpu.runnable && !cpu.on_gpu && gpu.on_gpu);
    assert!(cpu.est_secs > gpu.est_secs * 5.0);
    assert_eq!(fit(&m, &h1, &b1, 0, Some(9.0)).est_secs, 9.0, "a measurement wins");
}

#[test]
fn the_catalog_is_consistent() {
    let c = crate::catalog::Catalog::bundled().media;
    assert!(c.engine.assets.contains_key("vulkan-x64") && c.engine.assets.contains_key("cpu-x64"));
    for m in &c.models {
        assert!(!m.files.is_empty(), "{}", m.id);
        for f in &m.files {
            assert_eq!(f.sha256.len(), 64, "{} {}", m.id, f.file);
            assert!(f.url.starts_with("https://"));
        }
        match m.kind {
            // YuE2: its backbone and decoder, and its engine in the catalog.
            Kind::Music if m.file("yue").is_some() => assert!(m.file("yue_vae").is_some() && c.yue_engine.is_some(), "{}", m.id),
            // HeartMuLa: one file, and audio.cpp in the catalog.
            Kind::Music if m.file("heartmula").is_some() => assert!(m.files.len() == 1 && c.heartmula_engine.is_some(), "{}", m.id),
            // Only ace-synth runs: no planning model to download.
            Kind::Music => assert!(m.file("dit").is_some() && m.file("lm").is_none(), "{}", m.id),
            // A whole checkpoint (SDXL), or a diffusion model with its decoder.
            Kind::Image | Kind::Video => assert!(m.file("model").is_some() || (m.file("diffusion").is_some() && m.file("vae").is_some()), "{}", m.id),
            _ => {}
        }
        // A family opened to Hugging Face replaces a file the model has.
        if let Some(h) = &m.hub {
            assert!(m.file(&h.role).is_some() && !h.bases.is_empty() && h.min_size < h.max_size, "{}", m.id);
        }
    }
    // Shared files are listed identically wherever they appear.
    let mut seen = std::collections::HashMap::new();
    for m in &c.models {
        for f in &m.files {
            if let Some(h) = seen.insert((model_dir(&Paths { data: "d".into(), models: "m".into(), engines: "e".into(), logs: "l".into(), db: "x".into() }, m), f.file.clone()), f.sha256.clone()) {
                assert_eq!(h, f.sha256, "{} differs between models", f.file);
            }
        }
    }
}

#[test]
fn the_memory_budget_is_replaced_before_the_output() {
    let args: Vec<std::ffi::OsString> = ["-p", "x", "--max-vram", "9.0", "-o", "out.png"].iter().map(|s| s.into()).collect();
    let out: Vec<String> = with_flag(&args, "--max-vram", Some("4.5")).iter().map(|s| s.to_string_lossy().into_owned()).collect();
    assert_eq!(out, ["-p", "x", "--max-vram", "4.5", "-o", "out.png"]);
    let plain: Vec<std::ffi::OsString> = ["-p", "x", "-o", "o.png"].iter().map(|s| s.into()).collect();
    let added: Vec<String> = with_flag(&plain, "--max-vram", Some("-1")).iter().map(|s| s.to_string_lossy().into_owned()).collect();
    assert_eq!(added, ["-p", "x", "--max-vram", "-1", "-o", "o.png"], "before the output path");
}

/// The music model offered is the clearest that keeps up: XL on a
/// graphics card like the RTX 5070 (every word heard in tests, about 40
/// seconds a 30-second song), the clear-voice model on the processor.
#[test]
fn music_offers_the_clearest_model_that_keeps_up() {
    let pick = |vram: u64, cores: usize| {
        let (h, b) = hw(vram, cores);
        crate::catalog::Catalog::bundled()
            .media
            .models
            .into_iter()
            .filter(|m| m.kind == Kind::Music && m.can("music"))
            .filter(|m| fit(m, &h, &b, 0, None).est_secs <= 60.0)
            .max_by_key(|m| m.quality)
            .map(|m| m.id)
    };
    assert_eq!(pick(12, 32).as_deref(), Some("ace-step-1.5-xl"));
    assert_eq!(pick(0, 16).as_deref(), Some("ace-step-1.5-clear"));
    let (h, b) = hw(12, 32);
    let xl = fit(&model("ace-step-1.5-xl"), &h, &b, 0, None);
    assert!(xl.on_gpu && xl.est_secs <= 60.0, "{}", xl.est_secs);
}

/// A PC with 8 GB and no graphics card runs the small music model; the
/// other two are left out for memory, and the Studio says so per kind.
#[test]
fn music_models_left_out_are_counted_with_why() {
    let mut h = Hardware { cpu_cores: Some(8), cpu_threads: 16, avx2: true, ram_total: 8 * GIB, disk_free: 500 * GIB, ..Default::default() };
    let b = Budget::from_hardware(&h);
    let mut hidden = Vec::new();
    for id in ["ace-step-1.5", "ace-step-1.5-xl", "ace-step-1.5-small"] {
        let m = model(id);
        let f = fit(&m, &h, &b, m.size(), None);
        if !(f.runnable && f.disk_ok) {
            count_hidden(&mut hidden, m.kind, hidden_why(&m, &f));
        }
    }
    // Without the planning model's 2 GB, only XL needs more than 8 GB.
    assert_eq!(hidden, vec![Hidden { kind: Kind::Music, why: "memory", count: 1 }]);
    // Room for all of them but not on the disk: "disk".
    h.ram_total = 32 * GIB;
    h.disk_free = 6 * GIB;
    let b = Budget::from_hardware(&h);
    let xl = model("ace-step-1.5-xl");
    let f = fit(&xl, &h, &b, xl.size(), None);
    assert!(f.runnable && !f.disk_ok);
    assert_eq!(hidden_why(&xl, &f), "disk");
    let wan = model("wan2.2-ti2v-5b");
    for vram in [0, 4] {
        let (h, b) = hw(vram, 8);
        assert_eq!(hidden_why(&wan, &fit(&wan, &h, &b, 0, None)), "card", "{vram} GB");
    }
}

#[test]
fn ops_map_to_features() {
    assert_eq!(op_feature("restyle"), Some(Feature::Images));
    assert_eq!(op_feature("remove_background"), Some(Feature::Images));
    assert_eq!(op_feature("video"), Some(Feature::Video));
    assert_eq!(op_feature("narrate"), Some(Feature::Music));
    assert_eq!(op_feature("nonsense"), None);
}

#[test]
fn the_vocal_language_follows_the_choice() {
    use lyrics::Words;
    let sung = Words::Sung("[Verse]\nhola".into());
    assert_eq!(vocal_language(Some("es"), &sung).as_deref(), Some("es"));
    // Any language: the lyrics' own, always named (unnamed, words came out mumbled).
    assert_eq!(vocal_language(Some("auto"), &Words::Sung("[Verse]\nLa luna canta en el mar azul".into())).as_deref(), Some("es"));
    assert_eq!(vocal_language(None, &Words::Sung("[Verse]\nThe road is calling me".into())).as_deref(), Some("en"));
    assert_eq!(vocal_language(None, &Words::MusicModelWrites).as_deref(), Some("en"), "no words and no choice: English, not made-up words");
    assert_eq!(vocal_language(Some("ja"), &Words::MusicModelWrites).as_deref(), Some("ja"));
    assert_eq!(vocal_language(Some("fr"), &Words::Instrumental), None, "nothing sung");
}

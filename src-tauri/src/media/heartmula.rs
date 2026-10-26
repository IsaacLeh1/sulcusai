// SPDX-License-Identifier: AGPL-3.0-only
//! audio.cpp's HeartMuLa (3B, Apache-2.0, from Peking University and MuLa
//! Labs): a new song from style tags and lyrics in one run of
//! `audiocpp_cli` (Vulkan, any graphics card). It writes the song's audio
//! tokens, then its codec decodes them in 10 s pieces with the memory saver
//! on: in 30 s pieces the decoder wanted 5.5 GB at once and ran out on a
//! 12 GB card. The song ends with its lyrics, up to the length asked. On an
//! RTX 5070 a 144 s song took 86 s (10.4 GB at most) and 30 of its 31 lyric
//! words were heard. New songs only: covers and parts stay with ACE-Step.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::music::{Engine, Song};
use super::proc::{self, Run};

pub const EXE: &str = if cfg!(windows) { "audiocpp_cli.exe" } else { "audiocpp_cli" };

pub struct HeartRequest<'a> {
    /// The description, as HeartMuLa's tags (`tags_of`).
    pub caption: &'a str,
    /// Lyrics with their section tags.
    pub lyrics: &'a str,
    /// The most it may run, in seconds.
    pub seconds: f32,
    pub seed: i64,
    /// Songs made one after another, each from its own seed.
    pub versions: u32,
    pub model: &'a str,
    /// How freely it writes (its sampling temperature, 1 its own).
    pub temperature: Option<f32>,
    /// How closely it follows the tags (its guidance, 1.5 its own).
    pub guidance: Option<f32>,
}

/// A description as HeartMuLa's tags: short comma-separated words, lower
/// case, the singer said plainly ("male vocal"), long phrases left out.
pub fn tags_of(caption: &str) -> String {
    let mut tags: Vec<String> = Vec::new();
    for part in caption.split([',', '.', ';']) {
        let p = part.trim().to_lowercase();
        if p.is_empty() {
            continue;
        }
        let singer = if p.contains("female") || p.contains("woman") {
            Some("female vocal")
        } else if p.contains("male") || p.contains(" man") || p.starts_with("man") {
            Some("male vocal")
        } else {
            None
        };
        let tag = match singer {
            Some(s) if p.contains("vocal") || p.contains("voice") || p.contains("sung") || p.contains("singer") => s.to_string(),
            _ if p.split_whitespace().count() <= 4 => p,
            _ => continue,
        };
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags.truncate(14);
    tags.join(",")
}

/// Lyrics in HeartMuLa's section names: [Intro] [Verse] [Prechorus]
/// [Chorus] [Bridge] [Outro], without numbers or ways of singing ("[Verse 2
/// - whispered]" is "[Verse]"); other tags are kept as they are.
pub fn lyrics_for(lyrics: &str) -> String {
    lyrics
        .lines()
        .map(|l| {
            let t = l.trim();
            let Some(inner) = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')) else { return l.to_string() };
            let head = inner.split([' ', '-', ':', '(']).next().unwrap_or(inner).to_lowercase();
            let pre = inner.to_lowercase().replace([' ', '-'], "").starts_with("prechorus");
            match head.as_str() {
                _ if pre => "[Prechorus]".into(),
                "intro" => "[Intro]".into(),
                "verse" => "[Verse]".into(),
                "chorus" | "hook" | "refrain" => "[Chorus]".into(),
                "bridge" => "[Bridge]".into(),
                "outro" => "[Outro]".into(),
                _ => t.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How long a song takes, roughly: loading, then about 0.6 s a second of song.
fn expected_secs(seconds: f32) -> f64 {
    20.0 + 0.6 * seconds as f64
}

pub async fn make(e: &Engine<'_>, work: &Path, r: &HeartRequest<'_>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<Song>, String> {
    let exe = crate::engine::find_exe(e.dir, EXE).ok_or("HeartMuLa's engine isn't fully installed. Reinstall HeartMuLa from the Studio's Models list.")?;
    let versions = r.versions.clamp(1, 3);
    let tags = tags_of(r.caption);
    let mut songs = Vec::new();
    for v in 0..versions {
        let out = work.join(format!("song{v}.wav"));
        let mut args: Vec<std::ffi::OsString> = vec![
            "--task".into(),
            "gen".into(),
            "--family".into(),
            "heartmula".into(),
            "--model".into(),
            e.models.join(r.model).into(),
            "--backend".into(),
            "vulkan".into(),
            "--lyrics".into(),
            r.lyrics.into(),
            "--request-option".into(),
            format!("tags={tags}").into(),
            "--request-option".into(),
            format!("duration_sec={}", r.seconds.clamp(10.0, 360.0)).into(),
            "--request-option".into(),
            "codec_duration_sec=10".into(),
            "--session-option".into(),
            "heartmula.mem_saver=true".into(),
            "--seed".into(),
            (r.seed + v as i64).to_string().into(),
            "--out".into(),
            out.clone().into(),
        ];
        if let Some(t) = r.temperature {
            args.extend(["--temperature".into(), format!("{:.2}", t.clamp(0.3, 2.0)).into()]);
        }
        if let Some(g) = r.guidance {
            args.extend(["--guidance-scale".into(), format!("{:.2}", g.clamp(1.0, 5.0)).into()]);
        }
        // Its log says nothing while it works: the bar follows the time.
        let label = if versions > 1 { format!("Recording version {} of {versions}", v + 1) } else { "Recording".to_string() };
        let started = Instant::now();
        let finished = AtomicBool::new(false);
        let share = |f: f64| (v as f64 + f) / versions as f64;
        let ticker = async {
            while !finished.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
                let f = (started.elapsed().as_secs_f64() / expected_secs(r.seconds)).min(0.95);
                progress(&label, 0.05 + 0.9 * share(f));
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        };
        let run = async {
            let ran = proc::run(Run { exe: &exe, args, cwd: work, log: &e.logs.join("music-heartmula.log"), low_priority: e.low_priority, job: e.job }, cancel, |_: &str| {}).await;
            finished.store(true, Ordering::Relaxed);
            ran
        };
        let (ran, _) = tokio::join!(run, ticker);
        ran?;
        progress("Mixing", 0.05 + 0.9 * share(1.0));
        songs.push(to_mp3(&out, work, v)?);
    }
    Ok(songs)
}

/// The engine's WAV as an MP3 (320 kbps), with its length.
#[cfg(windows)]
fn to_mp3(wav: &Path, work: &Path, v: u32) -> Result<Song, String> {
    if !wav.is_file() {
        return Err("HeartMuLa didn't produce a song.".into());
    }
    let (samples, rate, channels) = super::audio_in::decode(wav)?;
    let ch = channels.max(1) as usize;
    let frames = samples.len() / ch;
    let stereo: Vec<i16> = (0..frames).flat_map(|i| [samples[i * ch], samples[i * ch + (ch > 1) as usize]]).map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    let mp3 = work.join(format!("song{v}.mp3"));
    super::mp4::mp3(&mp3, &stereo, rate)?;
    Ok(Song { mp3: std::fs::read(&mp3).map_err(|e| e.to_string())?, seconds: frames as f64 / rate as f64, lyrics: String::new() })
}

#[cfg(not(windows))]
fn to_mp3(_: &Path, _: &Path, _: u32) -> Result<Song, String> {
    Err("HeartMuLa isn't available on this system yet.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_become_short_tags() {
        let c = "A modern country song about a fresh start, male lead vocals, country, acoustic guitar, pedal steel, sung by a man, male voice throughout";
        assert_eq!(tags_of(c), "male vocal,country,acoustic guitar,pedal steel");
        assert_eq!(tags_of("Dreamy Synthwave, female lead vocals, sad mood. Lo-fi production"), "dreamy synthwave,female vocal,sad mood,lo-fi production");
    }

    #[test]
    fn sections_take_heartmulas_names() {
        let l = "[Verse 1 - whispered]\na\n\n[Pre-Chorus]\nb\n[Chorus - belting]\nc\n[Instrumental Break]\n[Outro]\nd";
        assert_eq!(lyrics_for(l), "[Verse]\na\n\n[Prechorus]\nb\n[Chorus]\nc\n[Instrumental Break]\n[Outro]\nd");
    }
}

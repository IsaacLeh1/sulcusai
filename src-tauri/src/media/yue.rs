// SPDX-License-Identifier: AGPL-3.0-only
//! yue2.cpp: YuE2 (3B; its weights CC BY-NC 4.0, so non-commercial) makes a
//! new song in one run of `yue-synth` from style tags and lyrics. It writes
//! the song's score first (melody and chords), then its sound. Its makers'
//! benchmark put it above Suno v6; here it heard 28 and 29 of 31 lyric words
//! in a 100 s country song (ACE-Step 1.5 XL: 28 and 30), exactly as long as
//! asked, in 39-71 s on an RTX 5070 (Vulkan). New songs only: covers, parts
//! and repaints stay with ACE-Step.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use super::music::{Engine, Song};
use super::proc::{self, Run};

pub const EXE: &str = if cfg!(windows) { "yue-synth.exe" } else { "yue-synth" };

pub struct YueRequest<'a> {
    /// Style tags, comma separated.
    pub style: &'a str,
    /// Lyrics with their section tags ("" lets it compose on its own).
    pub lyrics: &'a str,
    pub seconds: f32,
    pub seed: i64,
    /// Songs made at once (1 to 3), each with its own score.
    pub versions: u32,
    pub backbone: &'a str,
    pub vae: &'a str,
}

pub fn request_json(r: &YueRequest<'_>) -> Value {
    json!({
        "style": r.style,
        "lyrics": r.lyrics,
        "cot": "full",
        "duration": r.seconds,
        "lm_seed": r.seed,
        "seed": r.seed,
        "lm_batch_size": r.versions.clamp(1, 3),
        "output_format": "mp3",
        "mp3_bitrate": 320,
    })
}

/// How far along a run is, from its log: the score, the sound's codes, the
/// flow steps and the decoding.
#[derive(Default)]
struct Tracker {
    stage: u8,
    part: f64,
}

fn counter() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\[(AR\] Score|AR\] Semantic|NAR\] Step) (\d+)/(\d+)").unwrap())
}

impl Tracker {
    fn line(&mut self, l: &str, seconds: f32) -> bool {
        if let Some(c) = counter().captures(l) {
            let (n, total): (f64, f64) = (c[2].parse().unwrap_or(0.0), c[3].parse().unwrap_or(1.0));
            let (stage, part) = match &c[1] {
                // The score ends when the song does: about 12 tokens a second.
                s if s.ends_with("Score") => (0, n / (seconds as f64 * 12.0).max(1.0)),
                s if s.ends_with("Semantic") => (1, n / total.max(1.0)),
                _ => (2, n / total.max(1.0)),
            };
            self.stage = stage;
            self.part = part.min(1.0);
            return true;
        }
        if l.starts_with("[VAE] Track") && self.stage < 3 {
            self.stage = 3;
            return true;
        }
        false
    }

    fn label(&self) -> &'static str {
        ["Writing the melody", "Composing", "Recording", "Mixing"][self.stage as usize]
    }

    fn fraction(&self) -> f64 {
        match self.stage {
            0 => 0.05 + 0.15 * self.part,
            1 => 0.2 + 0.5 * self.part,
            2 => 0.7 + 0.2 * self.part,
            _ => 0.92,
        }
    }
}

pub async fn make(e: &Engine<'_>, work: &Path, r: &YueRequest<'_>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<Song>, String> {
    let exe = crate::engine::find_exe(e.dir, EXE).ok_or("The YuE2 engine isn't fully installed. Reinstall YuE2 from the Studio's Models list.")?;
    let request = work.join("song.json");
    std::fs::write(&request, request_json(r).to_string()).map_err(|e| e.to_string())?;
    let mut t = Tracker::default();
    progress(t.label(), t.fraction());
    let mut made_secs: Vec<f64> = Vec::new();
    proc::run(
        Run {
            exe: &exe,
            args: vec![
                "--model".into(),
                e.models.join(r.backbone).into(),
                "--vae".into(),
                e.models.join(r.vae).into(),
                "--request".into(),
                request.into(),
                "--out".into(),
                work.join("song.mp3").into(),
            ],
            cwd: work,
            log: &e.logs.join("music-yue.log"),
            low_priority: e.low_priority,
            job: e.job,
        },
        cancel,
        |l: &str| {
            // "[VAE] Tiled decode done: 5 tiles -> T_audio=4799936 (100.00s @ 48kHz)"
            if let Some(s) = super::music::decoded_secs(l) {
                made_secs.push(s);
            }
            if t.line(l, r.seconds) {
                progress(t.label(), t.fraction());
            }
        },
    )
    .await?;
    // One song lands on song.mp3; a batch numbers them song00.mp3, song10.mp3…
    let versions = r.versions.clamp(1, 3);
    let mut songs = Vec::new();
    for i in 0..versions {
        let name = if versions == 1 { "song.mp3".to_string() } else { format!("song{i}0.mp3") };
        let Ok(mp3) = std::fs::read(work.join(name)) else { continue };
        let seconds = made_secs.get(i as usize).copied().unwrap_or(r.seconds as f64);
        songs.push(Song { mp3, seconds, lyrics: r.lyrics.to_string() });
    }
    if songs.is_empty() {
        return Err("YuE2 didn't produce a song.".into());
    }
    Ok(songs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_the_song() {
        let r = YueRequest { style: "country, male vocal", lyrics: "[Verse]\nla", seconds: 90.0, seed: 7, versions: 2, backbone: "b.gguf", vae: "v.gguf" };
        let v = request_json(&r);
        assert_eq!((v["style"].as_str(), v["duration"].as_f64(), v["lm_batch_size"].as_u64(), v["mp3_bitrate"].as_u64()), (Some("country, male vocal"), Some(90.0), Some(2), Some(320)));
    }

    #[test]
    fn progress_follows_the_log() {
        let mut t = Tracker::default();
        assert!(t.line("[AR] Score 600/4096", 100.0));
        assert_eq!(t.label(), "Writing the melody");
        assert!(t.line("[AR] Semantic 1250/2500", 100.0));
        assert!((t.fraction() - 0.45).abs() < 1e-9);
        assert!(t.line("[NAR] Step 16/32, 417 ms", 100.0));
        assert_eq!(t.label(), "Recording");
        assert!(t.line("[VAE] Track 1/1: song 0 variation 0", 100.0));
        assert!(!t.line("[Store] Load LM: 900 ms", 100.0));
    }
}

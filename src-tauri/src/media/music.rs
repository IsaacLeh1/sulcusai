// SPDX-License-Identifier: AGPL-3.0-only
//! acestep.cpp: a song is two runs. `ace-lm` plans it (style details,
//! lyrics if asked, and the music as codes), then `ace-synth` renders the
//! codes to audio.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use super::proc::{self, Run};
use crate::engine::JobRef;

pub struct Song {
    pub mp3: Vec<u8>,
    pub seconds: f64,
    pub lyrics: String,
}

pub struct SongRequest<'a> {
    pub caption: &'a str,
    /// "" lets the music model write them; "[Instrumental]" for none.
    pub lyrics: &'a str,
    pub seconds: f32,
    pub seed: i64,
    pub language: Option<&'a str>,
    pub lm_file: &'a str,
    pub dit_file: &'a str,
}

pub struct Engine<'a> {
    pub dir: &'a Path,
    pub models: &'a Path,
    pub low_priority: bool,
    pub job: Option<&'a JobRef>,
    pub logs: &'a Path,
}

#[derive(Debug, Default)]
pub struct Tracker {
    stage: u8,
    done: f64,
    total: f64,
}

fn codes() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(\d+) total codes").unwrap())
}

fn dit_step() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\[DiT\] Step (\d+)/(\d+)").unwrap())
}

impl Tracker {
    fn new(seconds: f32) -> Tracker {
        Tracker { stage: 0, done: 0.0, total: seconds as f64 * 5.0 }
    }

    pub fn line(&mut self, l: &str) -> bool {
        if l.contains("[LM-Phase2]") {
            if let Some(c) = codes().captures(l) {
                self.stage = 1;
                self.done = c[1].parse().unwrap_or(0.0);
                return true;
            }
            if self.stage == 0 {
                self.stage = 1;
                self.done = 0.0;
                return true;
            }
        } else if let Some(c) = dit_step().captures(l) {
            self.stage = 2;
            self.done = c[1].parse().unwrap_or(0.0);
            self.total = c[2].parse().unwrap_or(8.0);
            return true;
        } else if l.starts_with("[VAE]") && self.stage < 3 {
            self.stage = 3;
            return true;
        }
        false
    }

    pub fn label(&self) -> &'static str {
        ["Writing", "Composing", "Recording", "Mixing"][self.stage as usize]
    }

    pub fn fraction(&self) -> f64 {
        let part = if self.total > 0.0 { (self.done / self.total).min(1.0) } else { 0.0 };
        match self.stage {
            0 => 0.05,
            1 => 0.1 + 0.25 * part,
            2 => 0.35 + 0.35 * part,
            _ => 0.8,
        }
    }
}

pub fn request_json(r: &SongRequest<'_>) -> Value {
    let mut v = json!({
        "caption": r.caption,
        "lyrics": r.lyrics,
        "duration": r.seconds,
        "seed": r.seed,
        "lm_model": r.lm_file,
        "synth_model": r.dit_file,
        "output_format": "mp3",
    });
    if let Some(l) = r.language.filter(|l| !l.is_empty() && *l != "auto") {
        v["vocal_language"] = json!(l);
    }
    v
}

pub async fn make(e: &Engine<'_>, work: &Path, r: &SongRequest<'_>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Song, String> {
    let exe = |name: &str| -> Result<PathBuf, String> {
        crate::engine::find_exe(e.dir, name).ok_or_else(|| "The music engine isn't fully installed. Reinstall the music model from the Studio's Models list.".to_string())
    };
    let plan = work.join("song.json");
    std::fs::write(&plan, request_json(r).to_string()).map_err(|e| e.to_string())?;
    let mut t = Tracker::new(r.seconds);
    progress(t.label(), t.fraction());
    let mut on_line = |l: &str| {
        if t.line(l) {
            progress(t.label(), t.fraction());
        }
    };
    proc::run(
        Run {
            exe: &exe(if cfg!(windows) { "ace-lm.exe" } else { "ace-lm" })?,
            args: vec!["--models".into(), e.models.into(), "--request".into(), plan.clone().into()],
            cwd: work,
            log: &e.logs.join("music-lm.log"),
            low_priority: e.low_priority,
            job: e.job,
        },
        cancel,
        &mut on_line,
    )
    .await?;
    let planned = work.join("song0.json");
    if !planned.exists() {
        return Err("The music model didn't finish planning the song.".into());
    }
    proc::run(
        Run {
            exe: &exe(if cfg!(windows) { "ace-synth.exe" } else { "ace-synth" })?,
            args: vec!["--models".into(), e.models.into(), "--request".into(), planned.clone().into()],
            cwd: work,
            log: &e.logs.join("music-synth.log"),
            low_priority: e.low_priority,
            job: e.job,
        },
        cancel,
        &mut on_line,
    )
    .await?;
    let mp3 = std::fs::read(work.join("song00.mp3")).map_err(|_| "The music model didn't produce a song.".to_string())?;
    let info: Value = std::fs::read_to_string(&planned).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    Ok(Song {
        mp3,
        seconds: info["duration"].as_f64().unwrap_or(r.seconds as f64),
        lyrics: info["lyrics"].as_str().unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_both_runs() {
        let mut t = Tracker::new(30.0);
        assert_eq!(t.label(), "Writing");
        assert!(t.line("[LM-Phase2] Step 100, 1 active, 101 total codes, 20.4 tok/s"));
        assert_eq!(t.label(), "Composing");
        assert!((t.fraction() - (0.1 + 0.25 * 101.0 / 150.0)).abs() < 1e-9);
        assert!(t.line("[DiT] Step 4/8 t=0.833"));
        assert!((t.fraction() - 0.525).abs() < 1e-9);
        assert!(t.line("[VAE] Backend: CPU, Weight buffer: 161.1 MB"));
        assert_eq!(t.label(), "Mixing");
    }

    #[test]
    fn request_names_the_models_and_language() {
        let r = SongRequest { caption: "folk", lyrics: "[Instrumental]", seconds: 30.0, seed: 3, language: Some("en"), lm_file: "lm.gguf", dit_file: "dit.gguf" };
        let v = request_json(&r);
        assert_eq!(v["lm_model"], "lm.gguf");
        assert_eq!(v["vocal_language"], "en");
        let auto = SongRequest { language: Some("auto"), ..r };
        assert!(request_json(&auto).get("vocal_language").is_none());
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! acestep.cpp: a song is two runs. `ace-lm` plans it (style details,
//! lyrics if asked, and the music as codes), then `ace-synth` renders the
//! codes to audio. Work on a song the person has (a cover in another style,
//! part of it made again, more after its end, a part added, one taken out)
//! goes straight to `ace-synth` with that song as its source.

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

#[derive(Default)]
pub struct SongRequest<'a> {
    pub caption: &'a str,
    /// "" lets the music model write them; "[Instrumental]" for none.
    pub lyrics: &'a str,
    pub seconds: f32,
    pub seed: i64,
    pub language: Option<&'a str>,
    pub lm_file: &'a str,
    pub dit_file: &'a str,
    /// Beats a minute (None: the music model picks).
    pub bpm: Option<u32>,
    /// "C major", "F# minor" (None: the music model picks).
    pub key: Option<&'a str>,
    /// "4/4", "3/4", "6/8" (None: the music model picks).
    pub time_signature: Option<&'a str>,
    /// The engine's task: text2music (a new song), cover, repaint, lego
    /// (add a part), extract (take one out) or complete.
    pub task: &'a str,
    /// The part added, taken out or completed with ("vocals", "drums").
    pub track: Option<&'a str>,
    /// What part of the source to make again, in seconds (repaint and
    /// extending: an end past the source's makes more after it).
    pub region: Option<(f32, f32)>,
    /// A cover: how much of the source it keeps (0 to 1).
    pub strength: Option<f32>,
    /// The song worked on (WAV or MP3).
    pub source: Option<&'a Path>,
    /// A song whose sound (voices and instruments) to follow.
    pub reference: Option<&'a Path>,
}

/// The parts the music engine knows by name, for adding, taking out or
/// completing ("vocals", "drums"…).
pub const PARTS: &[&str] = &["vocals", "backing_vocals", "drums", "bass", "guitar", "keyboard", "percussion", "strings", "synth", "fx", "brass", "woodwinds"];

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
        "seed": r.seed,
        "lm_model": r.lm_file,
        "synth_model": r.dit_file,
        "output_format": "mp3",
    });
    // A new song is as long as asked; work on a song follows the song.
    if r.source.is_none() {
        v["duration"] = json!(r.seconds);
    }
    if let Some(l) = r.language.filter(|l| !l.is_empty() && *l != "auto") {
        v["vocal_language"] = json!(l);
    }
    if let Some(b) = r.bpm.filter(|b| (30..=300).contains(b)) {
        v["bpm"] = json!(b);
    }
    if let Some(k) = r.key.filter(|k| !k.trim().is_empty()) {
        v["keyscale"] = json!(k.trim());
    }
    if let Some(t) = r.time_signature.filter(|t| !t.trim().is_empty()) {
        // The engine takes the beats in a bar ("4" for 4/4), as ACE-Step was trained.
        v["timesignature"] = json!(t.split('/').next().unwrap_or(t).trim());
    }
    if !r.task.is_empty() && r.task != "text2music" {
        v["task_type"] = json!(r.task);
    }
    if let Some(t) = r.track.filter(|t| !t.is_empty()) {
        v["track"] = json!(t);
    }
    if let Some((a, b)) = r.region {
        v["repainting_start"] = json!(a);
        v["repainting_end"] = json!(b);
    }
    if let Some(s) = r.strength {
        v["audio_cover_strength"] = json!(s.clamp(0.0, 1.0));
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
    // Work on a song the person has: the song itself is the plan's music,
    // so the planning run is skipped.
    let planned = if r.source.is_some() {
        plan.clone()
    } else {
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
        planned
    };
    let mut synth_args: Vec<std::ffi::OsString> = vec!["--models".into(), e.models.into(), "--request".into(), planned.clone().into()];
    if let Some(s) = r.source {
        synth_args.extend(["--src-audio".into(), s.into()]);
    }
    if let Some(s) = r.reference {
        synth_args.extend(["--ref-audio".into(), s.into()]);
    }
    proc::run(
        Run {
            exe: &exe(if cfg!(windows) { "ace-synth.exe" } else { "ace-synth" })?,
            args: synth_args,
            cwd: work,
            log: &e.logs.join("music-synth.log"),
            low_priority: e.low_priority,
            job: e.job,
        },
        cancel,
        &mut on_line,
    )
    .await?;
    // Named after the request file: song00.mp3 from song0.json, song.mp3-like
    // names from song.json (work on a song); the first MP3 it wrote.
    let out = ["song00.mp3", "song0.mp3", "song.mp3"].iter().map(|n| work.join(n)).find(|p| p.exists()).or_else(|| {
        std::fs::read_dir(work).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "mp3"))
    });
    let mp3 = std::fs::read(out.ok_or("The music model didn't produce a song.")?).map_err(|_| "The music model didn't produce a song.".to_string())?;
    let info: Value = std::fs::read_to_string(&planned).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    Ok(Song {
        mp3,
        seconds: info["duration"].as_f64().unwrap_or(r.seconds as f64),
        // What was sung: the plan's lyrics (the music model's own, when it
        // wrote them), else the ones it was given.
        lyrics: info["lyrics"].as_str().unwrap_or(r.lyrics).to_string(),
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
        let r = SongRequest { caption: "folk", lyrics: "[Instrumental]", seconds: 30.0, seed: 3, language: Some("en"), lm_file: "lm.gguf", dit_file: "dit.gguf", ..Default::default() };
        let v = request_json(&r);
        assert_eq!(v["lm_model"], "lm.gguf");
        assert_eq!(v["vocal_language"], "en");
        assert_eq!(v["duration"], 30.0);
        assert!(v.get("task_type").is_none() && v.get("bpm").is_none());
        let auto = SongRequest { language: Some("auto"), ..Default::default() };
        assert!(request_json(&auto).get("vocal_language").is_none());
    }

    /// Tempo, key and time signature as the engine reads them; work on a
    /// song names its task and part, and its length is the song's.
    #[test]
    fn the_musical_settings_and_tasks_reach_the_engine() {
        let r = SongRequest { caption: "jazz", bpm: Some(92), key: Some("F# minor"), time_signature: Some("6/8"), seconds: 60.0, ..Default::default() };
        let v = request_json(&r);
        assert_eq!((v["bpm"].as_u64(), v["keyscale"].as_str(), v["timesignature"].as_str()), (Some(92), Some("F# minor"), Some("6")));
        assert_eq!(request_json(&SongRequest { bpm: Some(1000), ..Default::default() }).get("bpm"), None, "out of range");
        let src = Path::new("song.wav");
        let add = SongRequest { task: "lego", track: Some("drums"), source: Some(src), seconds: 60.0, ..Default::default() };
        let v = request_json(&add);
        assert_eq!((v["task_type"].as_str(), v["track"].as_str()), (Some("lego"), Some("drums")));
        assert!(v.get("duration").is_none(), "the song's own length");
        let redo = SongRequest { task: "repaint", region: Some((30.0, 45.0)), source: Some(src), ..Default::default() };
        let v = request_json(&redo);
        assert_eq!((v["repainting_start"].as_f64(), v["repainting_end"].as_f64()), (Some(30.0), Some(45.0)));
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! acestep.cpp: a song is one run of `ace-synth`, from its description,
//! lyrics and settings (tempo, key, time signature, length); its planning
//! model `ace-lm` isn't used (see `make`). Work on a song the person has (a
//! cover in another style, part of it made again, more after its end, a part
//! added, one taken out) gives that song to `ace-synth` as its source.

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

/// The engine's MP3s: 320 kbps (its default 128 smeared breaths, cymbals
/// and reverb). The songs are kept as the music model made them: a voice
/// part separated, worked on and mixed back sounded worse (`stems`).
const MP3_KBPS: u32 = 320;

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
    /// How many versions to make at once (1 to 3), to keep the best.
    pub versions: u32,
    /// The planning model (ace-lm's GGUF) that writes the song's music as
    /// audio codes before it's rendered, as ACE-Step 1.5 is meant to run;
    /// empty renders from noise alone. A new song only.
    pub planner: &'a str,
    /// The planner's sampling temperature (default 0.85): higher wanders more.
    pub lm_temperature: Option<f32>,
    /// How closely the planner follows the description (its guidance, default 2).
    pub lm_cfg: Option<f32>,
    /// What the song shouldn't sound like (the planner's negative prompt).
    pub negative: Option<&'a str>,
    /// How closely the description and lyrics are followed (None: as
    /// `guidance` picks for the model).
    pub guidance: Option<f32>,
    /// A trained adapter (a custom model, `train`): the folder of adapters,
    /// the one by name, and how strongly (1 as trained). It must match the
    /// model's size (2B): the engine merges it in when it loads the model.
    pub adapters: Option<&'a Path>,
    pub adapter: Option<&'a str>,
    pub adapter_scale: Option<f32>,
}

/// Tempo, key and time signature for a new song, from what the person set
/// and, for the rest, from its description: the music model's planner wrote
/// them itself when they were missing, and on this engine that text often
/// came out as garbage ("caption: 202 202 202…"), which broke the plan file
/// so the song failed. Given all of them, it only plans the music.
pub fn settle(caption: &str, bpm: Option<u32>, key: Option<&str>, time_signature: Option<&str>, seed: i64) -> (u32, String, String) {
    let c = caption.to_lowercase();
    let has = |w: &str| c.split(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '&').any(|x| x == w) || (w.contains(' ') && c.contains(w));
    // The first match wins: specific styles before general words.
    const TEMPOS: &[(&str, u32)] = &[
        ("drum and bass", 172), ("dnb", 172), ("punk", 170), ("metal", 140), ("trap", 140), ("dubstep", 140), ("techno", 128), ("house", 124),
        ("edm", 128), ("disco", 120), ("k-pop", 120), ("synthwave", 110), ("funk", 105), ("pop", 110), ("rock", 120), ("country", 100),
        ("folk", 100), ("latin", 100), ("reggaeton", 95), ("hip hop", 90), ("hip-hop", 90), ("rap", 90), ("boom bap", 90), ("r&b", 85),
        ("soul", 85), ("gospel", 80), ("reggae", 80), ("lo-fi", 80), ("lofi", 80), ("blues", 80), ("jazz", 120), ("swing", 140),
        ("waltz", 90), ("classical", 90), ("soundtrack", 90), ("cinematic", 90), ("ambient", 70), ("ballad", 70),
    ];
    const PACE: &[(&str, u32)] = &[("very fast", 170), ("fast", 140), ("energetic", 128), ("upbeat", 120), ("relaxed", 90), ("calm", 80), ("slow", 70)];
    let bpm = bpm
        .filter(|b| (30..=300).contains(b))
        .or_else(|| PACE.iter().find(|(w, _)| has(w)).map(|(_, b)| *b))
        .or_else(|| TEMPOS.iter().find(|(w, _)| has(w)).map(|(_, b)| *b))
        .unwrap_or(100);
    let key = key.map(str::trim).filter(|k| !k.is_empty()).map(str::to_string).unwrap_or_else(|| {
        let minor = ["sad", "dark", "angry", "nostalgic", "melancholy", "melancholic", "haunting", "minor", "moody", "epic"].iter().any(|w| has(w));
        // A common key, the same for the same seed.
        let roots = if minor { ["A", "E", "D", "C", "G", "B"] } else { ["C", "G", "D", "A", "F", "E"] };
        format!("{} {}", roots[(seed.unsigned_abs() % 6) as usize], if minor { "minor" } else { "major" })
    });
    let time = time_signature.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| {
        if has("waltz") || c.contains("3/4") {
            "3/4".into()
        } else if c.contains("6/8") {
            "6/8".into()
        } else {
            "4/4".into()
        }
    });
    (bpm, key, time)
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
        "synth_model": r.dit_file,
        "output_format": "mp3",
        "mp3_bitrate": MP3_KBPS,
    });
    // A new song is as long as asked; work on a song follows the song.
    if r.source.is_none() {
        v["duration"] = json!(r.seconds);
    }
    if let Some(l) = r.language.filter(|l| !l.is_empty() && *l != "auto") {
        v["vocal_language"] = json!(l);
    }
    if !r.lm_file.is_empty() {
        v["lm_model"] = json!(r.lm_file);
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
    if let Some(a) = r.adapter.filter(|a| !a.is_empty() && r.adapters.is_some()) {
        v["adapter"] = json!(a);
        v["adapter_scale"] = json!(r.adapter_scale.unwrap_or(1.0).clamp(0.0, 2.0));
    }
    if planned(r) {
        // Each version gets a plan of its own (different music, not only
        // another take of the same), and the description stays as written.
        v["lm_model"] = json!(r.planner);
        v["use_cot_caption"] = json!(false);
        v["lm_batch_size"] = json!(r.versions.clamp(1, 3));
        if let Some(t) = r.lm_temperature {
            v["lm_temperature"] = json!(t.clamp(0.1, 2.0));
        }
        if let Some(c) = r.lm_cfg {
            v["lm_cfg_scale"] = json!(c.clamp(1.0, 5.0));
        }
        if let Some(n) = r.negative.map(str::trim).filter(|n| !n.is_empty()) {
            v["lm_negative_prompt"] = json!(n);
        }
    } else if r.versions > 1 {
        v["synth_batch_size"] = json!(r.versions.min(3));
    }
    if let Some(g) = r.guidance {
        v["guidance_scale"] = json!(g.clamp(1.0, 15.0));
    } else if !r.lyrics.is_empty() && r.lyrics != "[Instrumental]" && r.source.is_none() {
        // Words to sing: the lyrics followed more closely (they were heard
        // better across seeds at 3 than at the turbo model's own 1).
        v["guidance_scale"] = json!(3.0);
    }
    v
}

/// A distilled ("turbo") music model: 8 steps with the guidance built in.
/// The others (SFT, Base) take 50 steps, and their makers run them with
/// guidance.
pub fn is_turbo(dit_file: &str) -> bool {
    dit_file.to_lowercase().contains("turbo")
}

/// Base and SFT's guidance when nothing else is asked: as their makers run them.
pub const FULL_GUIDANCE: f32 = 7.0;

/// How closely a song follows its description and lyrics (the guidance
/// scale), from Style influence (0 to 1, a half the usual): a turbo model
/// 1 to 5, Base and SFT 3 to 11. None asked: Base and SFT `FULL_GUIDANCE`,
/// a turbo model its own (`request_json` raises it for words to sing).
pub fn guidance(dit_file: &str, influence: Option<f32>) -> Option<f32> {
    let turbo = is_turbo(dit_file);
    match influence {
        Some(x) => {
            let x = x.clamp(0.0, 1.0);
            Some(if turbo { 1.0 + 4.0 * x } else { 3.0 + 8.0 * x })
        }
        None if !turbo => Some(FULL_GUIDANCE),
        None => None,
    }
}

/// Whether a song is planned by ace-lm first: a new song with a planner.
fn planned(r: &SongRequest<'_>) -> bool {
    !r.planner.is_empty() && r.source.is_none() && (r.task.is_empty() || r.task == "text2music")
}

pub async fn make(e: &Engine<'_>, work: &Path, r: &SongRequest<'_>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<Song>, String> {
    let exe = |name: &str| -> Result<PathBuf, String> {
        crate::engine::find_exe(e.dir, name).ok_or_else(|| "The music engine isn't fully installed. Reinstall the music model from the Studio's Models list.".to_string())
    };
    // One run of ace-synth, from the lyrics, the description and the
    // settings; with a planner, ace-lm first writes each version's music as
    // audio codes (everything else given, it only plans the music). Without
    // one, a song is rendered from noise. The planner was taken out once: on
    // an earlier engine its plan ended where it liked (40 s asked came out
    // at 23 s) and squeezed the words until none were heard.
    let request = work.join("song.json");
    std::fs::write(&request, request_json(r).to_string()).map_err(|e| e.to_string())?;
    let mut t = Tracker::new(r.seconds);
    progress(t.label(), t.fraction());
    let mut made_secs: Vec<f64> = Vec::new();
    let plans: Vec<PathBuf> = if planned(r) {
        proc::run(
            Run {
                exe: &exe(if cfg!(windows) { "ace-lm.exe" } else { "ace-lm" })?,
                args: vec!["--models".into(), e.models.into(), "--request".into(), request.clone().into()],
                cwd: work,
                log: &e.logs.join("music-plan.log"),
                low_priority: e.low_priority,
                job: e.job,
            },
            cancel,
            |l: &str| {
                if t.line(l) {
                    progress(t.label(), t.fraction());
                }
            },
        )
        .await?;
        // request.json plans into request0.json, request1.json…
        let plans: Vec<PathBuf> = (0..r.versions.clamp(1, 3)).map(|i| work.join(format!("song{i}.json"))).filter(|p| p.is_file()).collect();
        if plans.is_empty() {
            return Err("The music model's planner didn't write a plan.".into());
        }
        for p in &plans {
            if let Some(s) = planned_secs(p) {
                crate::applog::line("media", &format!("Planned a song of {s:.0} s ({:.0} s asked)", r.seconds));
            }
        }
        plans
    } else {
        Vec::new()
    };
    let mut args: Vec<std::ffi::OsString> = vec!["--models".into(), e.models.into(), "--request".into()];
    if plans.is_empty() {
        args.push(request.clone().into());
    } else {
        args.extend(plans.iter().map(|p| p.clone().into_os_string()));
    }
    if let Some(s) = r.source {
        args.extend(["--src-audio".into(), s.into()]);
    }
    if let Some(s) = r.reference {
        args.extend(["--ref-audio".into(), s.into()]);
    }
    if let (Some(dir), Some(_)) = (r.adapters, r.adapter) {
        args.extend(["--adapters".into(), dir.into()]);
    }
    proc::run(
        Run {
            exe: &exe(if cfg!(windows) { "ace-synth.exe" } else { "ace-synth" })?,
            args,
            cwd: work,
            log: &e.logs.join("music-synth.log"),
            low_priority: e.low_priority,
            job: e.job,
        },
        cancel,
        |l: &str| {
            if let Some(s) = decoded_secs(l) {
                made_secs.push(s);
            }
            if t.line(l) {
                progress(t.label(), t.fraction());
            }
        },
    )
    .await?;
    // Named after the request file: song.json writes song0.mp3, song1.mp3…;
    // planned, song0.json writes song00.mp3, song1.json song10.mp3.
    let mut songs = Vec::new();
    for i in 0..r.versions.clamp(1, 3) {
        let name = if plans.is_empty() { format!("song{i}.mp3") } else { format!("song{i}0.mp3") };
        let Ok(mp3) = std::fs::read(work.join(name)) else { continue };
        let seconds = made_secs.get(i as usize).copied().unwrap_or(r.seconds as f64);
        songs.push(Song { mp3, seconds, lyrics: r.lyrics.to_string() });
    }
    if songs.is_empty() {
        return Err("The music model didn't produce a song.".into());
    }
    Ok(songs)
}

/// How long a plan's music is: its audio codes, 5 a second.
fn planned_secs(plan: &Path) -> Option<f64> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(plan).ok()?).ok()?;
    let codes = v["audio_codes"].as_str()?;
    let n = codes.split(',').filter(|c| !c.trim().is_empty()).count();
    (n > 0).then(|| n as f64 / 5.0)
}

/// The length the synth says it wrote: "[VAE] Decoded: … (23.44s @ 48kHz)".
pub(crate) fn decoded_secs(line: &str) -> Option<f64> {
    if !line.starts_with("[VAE]") {
        return None;
    }
    let at = line.find("s @ 48kHz")?;
    let start = line[..at].rfind('(')? + 1;
    line[start..at].trim().parse().ok()
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
    /// The length the synth wrote.
    #[test]
    fn the_real_length_is_read() {
        assert_eq!(decoded_secs("[VAE] Decoded: T_latent=586 -> T_audio=1125120 (23.44s @ 48kHz)"), Some(23.44));
        assert_eq!(decoded_secs("[VAE] Tiled decode done: 5 tiles -> T_audio=8171520 (170.24s @ 48kHz)"), Some(170.24));
        assert_eq!(decoded_secs("[DiT] Step 4/8"), None);
    }

    /// Missing tempo, key and time signature are picked from the description.
    #[test]
    fn a_new_song_gets_all_its_settings() {
        assert_eq!(settle("upbeat pop about summer", None, None, None, 0), (120, "C major".into(), "4/4".into()));
        let (b, k, t) = settle("sad slow piano ballad", None, None, None, 1);
        assert_eq!((b, k.ends_with("minor"), t.as_str()), (70, true, "4/4"));
        assert_eq!(settle("a viennese waltz", None, None, None, 0).2, "3/4");
        assert_eq!(settle("drum and bass roller", None, None, None, 0).0, 172);
        assert_eq!(settle("anything", Some(92), Some("F# minor"), Some("6/8"), 0), (92, "F# minor".into(), "6/8".into()));
        assert_eq!(settle("rap over a boom bap beat", None, None, None, 0).0, 90);
    }

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

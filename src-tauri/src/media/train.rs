// SPDX-License-Identifier: AGPL-3.0-only
//! Custom models (Suno's): ACE-Step 1.5 Base taught the person's own songs,
//! as a LoRA the music engine merges in when it loads (`--adapters`), with
//! ACE-Step's own trainer (MIT, Python; its upstream copy of Side-Step's
//! "fixed" trainer). The first time, uv (one program) sets up its Python and
//! packages (about 6 GB, PyTorch for NVIDIA cards) beside the engines, and
//! the Base model's training files (6 GB) come to the models folder.
//!
//! A run: each song's words are heard by speech recognition and its style
//! is the person's description, with the model's name as its trigger word;
//! the songs are turned into the trainer's tensors, then trained (LoRA rank
//! 64, alpha 128, the Base model's 50-step schedule). On an RTX 5070 six
//! songs of up to two minutes took 3 s an epoch at 5.2 GB; the adapter
//! (84 MB) merged into Base's GGUF in 9 s. It needs an NVIDIA card: the
//! trainer's PyTorch is built for CUDA.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::catalog::Asset;
use crate::crypto::Cipher;
use crate::db;
use crate::paths::Paths;

const KEY: &str = "music_custom_models";
/// The music model a custom model plays on: the trainer's own model.
pub const BASE_MODEL: &str = "ace-step-1.5-base";

/// ACE-Step's trainer and what it needs, from the catalog.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrainerCatalog {
    pub name: String,
    pub license: String,
    /// The trainer's source revision.
    pub build: String,
    /// uv, which sets up the trainer's Python and packages.
    pub uv: Asset,
    /// ACE-Step 1.5's source at `build` (it holds the trainer and its locked packages).
    pub source: Asset,
    /// The Base model's training files, by their path under the checkpoints folder.
    pub checkpoints: Vec<TrainerFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrainerFile {
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// What a training job is given.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct TrainSpec {
    pub name: String,
    /// Their songs' style in words ("warm acoustic country, male voice").
    pub description: String,
    /// Song files on this PC.
    pub files: Vec<String>,
    /// Songs from the gallery.
    pub songs: Vec<String>,
}

/// A trained model.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct CustomModel {
    pub id: String,
    pub name: String,
    /// The word its songs' descriptions start with, as in training.
    pub trigger: String,
    pub description: String,
    pub songs: usize,
    pub epochs: u32,
    pub minutes: f64,
    pub created_at: i64,
}

pub fn list(conn: &Connection, c: &Cipher) -> Vec<CustomModel> {
    let mut all: Vec<CustomModel> = db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    all
}

fn store(conn: &Connection, c: &Cipher, all: &[CustomModel]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

pub fn get(conn: &Connection, c: &Cipher, id: &str) -> Option<CustomModel> {
    list(conn, c).into_iter().find(|m| m.id == id)
}

pub fn add(conn: &Connection, c: &Cipher, m: CustomModel) -> Result<(), String> {
    let mut all = list(conn, c);
    all.retain(|o| o.id != m.id);
    all.push(m);
    store(conn, c, &all)
}

/// Forgets a custom model and deletes its adapter.
pub fn remove(conn: &Connection, c: &Cipher, paths: &Paths, id: &str) -> Result<(), String> {
    if !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') || id.is_empty() {
        return Err("Bad model id.".into());
    }
    let mut all = list(conn, c);
    all.retain(|m| m.id != id);
    store(conn, c, &all)?;
    let _ = std::fs::remove_dir_all(adapters_dir(paths).join(id));
    Ok(())
}

/// Where trained adapters are kept, one folder each (the music engine's `--adapters`).
pub fn adapters_dir(paths: &Paths) -> PathBuf {
    paths.models.join("media").join("music").join("adapters")
}

/// The Base model's training files.
pub fn checkpoints_dir(paths: &Paths) -> PathBuf {
    paths.models.join("media").join("trainer")
}

/// The trainer's source (and its Python packages, in `.venv`).
pub fn source_dir(paths: &Paths, t: &TrainerCatalog) -> PathBuf {
    paths.engines.join(format!("acestep-trainer-{}", t.build))
}

/// A trigger word from a name: its letters and digits, lower case ("Ike's
/// Songs" is "ikessongs"), so the descriptions in training and in use match.
pub fn trigger_of(name: &str) -> String {
    let t: String = name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    if t.is_empty() { "mystyle".into() } else { t.chars().take(24).collect() }
}

/// Training length by how many songs (the trainer's guide: few songs need
/// more passes over them).
pub fn epochs_for(songs: usize) -> u32 {
    match songs {
        0..=10 => 300,
        11..=30 => 150,
        _ => 80,
    }
}

/// Words speech recognition heard, as lyrics: a line each, four to a
/// section. Fewer than eight words is an instrumental.
pub fn lyrics_from(lines: &[String]) -> Option<String> {
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| l.chars().any(char::is_alphabetic)).collect();
    if lines.iter().map(|l| l.split_whitespace().count()).sum::<usize>() < 8 {
        return None;
    }
    let mut out = Vec::new();
    for (i, chunk) in lines.chunks(4).enumerate() {
        out.push(if i % 2 == 0 { "[Verse]".to_string() } else { "[Chorus]".to_string() });
        out.extend(chunk.iter().map(|l| l.to_string()));
        out.push(String::new());
    }
    Some(out.join("\n").trim_end().to_string())
}

/// One song in the trainer's dataset.
pub struct Sample {
    pub file: PathBuf,
    pub lyrics: Option<String>,
    pub seconds: f64,
}

/// The trainer's dataset JSON (ACE-Step's format), every song with the
/// person's description and the trigger word in front.
pub fn dataset_json(samples: &[Sample], description: &str, trigger: &str) -> Value {
    let rows: Vec<Value> = samples
        .iter()
        .map(|s| {
            json!({
                "audio_path": s.file.to_string_lossy(),
                "filename": s.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                "caption": description,
                "lyrics": s.lyrics.clone().unwrap_or_else(|| "[Instrumental]".into()),
                "duration": s.seconds.round(),
                "is_instrumental": s.lyrics.is_none(),
                "custom_tag": trigger,
            })
        })
        .collect();
    json!({ "metadata": { "name": trigger, "custom_tag": trigger, "tag_position": "prepend", "genre_ratio": 0, "all_instrumental": samples.iter().all(|s| s.lyrics.is_none()) }, "samples": rows })
}

/// The trainer's arguments for one step: turning the songs into tensors
/// (`preprocess`), or training on them.
pub fn trainer_args(source: &Path, checkpoints: &Path, job: &Path, epochs: u32, preprocess: bool) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["run".into(), "--frozen".into(), "--no-sync".into(), "--project".into(), source.into(), "python".into(), source.join("train.py").into()];
    a.extend(["--plain", "--yes", "fixed"].map(OsString::from));
    a.extend(["--checkpoint-dir".into(), checkpoints.into(), "--model-variant".into(), "base".into()]);
    a.extend(["--dataset-dir".into(), job.join("tensors").into(), "--output-dir".into(), job.join("out").into()]);
    if preprocess {
        a.extend(["--preprocess".into(), "--audio-dir".into(), job.join("audio").into(), "--dataset-json".into(), job.join("dataset.json").into(), "--tensor-output".into(), job.join("tensors").into(), "--max-duration".into(), "240".into()]);
    } else {
        let e = epochs.to_string();
        a.extend(["--epochs", &e, "--save-every", &e, "--rank", "64", "--alpha", "128", "--lr", "1e-4", "--shift", "1.0", "--num-inference-steps", "50"].map(OsString::from));
    }
    a
}

/// The environment the trainer runs in: uv's Python, packages and cache
/// beside the trainer, nothing fetched while it runs.
pub fn trainer_env(paths: &Paths) -> Vec<(&'static str, OsString)> {
    let base = paths.engines.join("trainer-uv");
    vec![
        ("UV_CACHE_DIR", base.join("cache").into()),
        ("UV_PYTHON_INSTALL_DIR", base.join("python").into()),
        ("UV_PYTHON_PREFERENCE", "only-managed".into()),
        ("PYTHONIOENCODING", "utf-8".into()),
        ("PYTHONUTF8", "1".into()),
        ("HF_HUB_OFFLINE", "1".into()),
        ("TRANSFORMERS_OFFLINE", "1".into()),
    ]
}

/// How far a run is, from a line of the trainer's output: songs turned
/// into tensors, then epochs. (stage, done, of)
pub fn progress_of(line: &str, songs: usize) -> Option<(&'static str, u32, u32)> {
    if line.contains("Pass 1 OK") {
        return Some(("pass1", 0, songs as u32));
    }
    if line.contains("Pass 2 OK") {
        return Some(("pass2", 0, songs as u32));
    }
    let rest = line.split("Epoch ").nth(1)?;
    let (n, of) = rest.split_whitespace().next()?.split_once('/')?;
    Some(("epoch", n.parse().ok()?, of.trim_end_matches(',').parse().ok()?))
}

/// Whether the trainer and its packages are set up.
pub fn ready(paths: &Paths, t: &TrainerCatalog) -> bool {
    source_dir(paths, t).join(".venv").join("Scripts").join("python.exe").is_file() && t.checkpoints.iter().all(|f| checkpoints_dir(paths).join(&f.file).is_file())
}

/// How many bytes setting it up downloads, roughly (the packages' share is
/// an estimate: uv fetches them by their lock file).
pub fn setup_bytes(t: &TrainerCatalog) -> u64 {
    t.uv.size + t.source.size + t.checkpoints.iter().map(|f| f.size).sum::<u64>() + 3_000_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_trigger_words() {
        assert_eq!(trigger_of("Ike's Songs 2"), "ikessongs2");
        assert_eq!(trigger_of("  "), "mystyle");
    }

    #[test]
    fn heard_words_become_lyrics() {
        let l: Vec<String> = ["Morning light on the window", "Coffee warm in my hands", "Every road is a new start", "I can feel it again", "Sing it loud", "♪"].iter().map(|s| s.to_string()).collect();
        assert_eq!(lyrics_from(&l).unwrap(), "[Verse]\nMorning light on the window\nCoffee warm in my hands\nEvery road is a new start\nI can feel it again\n\n[Chorus]\nSing it loud");
        assert_eq!(lyrics_from(&["oh yeah".to_string()]), None, "a few words is an instrumental");
    }

    #[test]
    fn the_trainer_says_how_far_it_is() {
        assert_eq!(progress_of("2026-10-26 08:33:14 [INFO] acestep.training_v2.preprocess: [Side-Step] Pass 2 OK: noise-11.tmp", 6), Some(("pass2", 0, 6)));
        assert_eq!(progress_of("[OK] Epoch 2/300 in 3.1s, Loss: 0.9328", 6), Some(("epoch", 2, 300)));
        assert_eq!(progress_of("[INFO] Optimizer: adamw", 6), None);
    }

    #[test]
    fn the_dataset_marks_songs_with_the_trigger() {
        let s = [Sample { file: PathBuf::from(r"C:\j\audio\01.mp3"), lyrics: Some("[Verse]\nla la la".into()), seconds: 99.6 }, Sample { file: PathBuf::from(r"C:\j\audio\02.wav"), lyrics: None, seconds: 60.0 }];
        let v = dataset_json(&s, "warm acoustic country", "ikessongs");
        assert_eq!(v["metadata"]["custom_tag"], "ikessongs");
        assert_eq!(v["samples"][0]["filename"], "01.mp3");
        assert_eq!((v["samples"][1]["is_instrumental"].as_bool(), v["samples"][1]["lyrics"].as_str()), (Some(true), Some("[Instrumental]")));
        assert_eq!(v["samples"][0]["duration"].as_f64(), Some(100.0));
    }
}

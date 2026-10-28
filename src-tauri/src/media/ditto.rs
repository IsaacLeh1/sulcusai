// SPDX-License-Identifier: AGPL-3.0-only
//! Quick talking pictures (`talk`): Ditto (Ant Group, Apache-2.0) moves a
//! picture's face and lips to a voice at 25 frames a second, in about the
//! time the voice lasts plus a minute, where Wan 2.2 S2V (`speak`) takes
//! half an hour for every 5 seconds. It turns the face more than it
//! paints, so the picture stays as it was: no new background, hands or
//! clothes.
//!
//! It runs on PyTorch for NVIDIA cards, set up the first time by uv in its
//! own folder beside the engines, sharing the music trainer's uv and
//! package cache (the same PyTorch build), with its model files (2.3 GB)
//! from Hugging Face. `scripts/ditto` packs its source with SulcusAI's
//! runner (`talk.py`) and locked packages. On an RTX 5070 a 15.8 s voice
//! took 72 s at 1432x1432 and about 2 GB of the card.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::Asset;
use crate::paths::Paths;

use super::train::TrainerFile;

/// The longest voice it talks to (it makes the whole video in one go).
pub const LONGEST: f64 = 300.0;
/// Its settings file, among its model files.
const CFG: &str = "ditto_cfg/v0.4_hubert_cfg_pytorch.pkl";
/// Its models' folder, among its model files.
const MODELS: &str = "ditto_pytorch";

/// Ditto and what it needs, from the catalog.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DittoCatalog {
    pub name: String,
    pub license: String,
    /// Its source revision (short).
    pub build: String,
    /// uv, which sets up its Python and packages (the music trainer's).
    pub uv: Asset,
    /// Ditto's source at `build`, with SulcusAI's runner and locked packages.
    pub source: Asset,
    /// Its model files and settings, by their path under its models folder.
    pub files: Vec<TrainerFile>,
}

/// Its source (and its Python packages, in `.venv`).
pub fn source_dir(paths: &Paths, d: &DittoCatalog) -> PathBuf {
    paths.engines.join(format!("ditto-{}", d.build))
}

/// Its model files.
pub fn models_dir(paths: &Paths) -> PathBuf {
    paths.models.join("media").join("ditto")
}

/// Whether it and its packages and files are set up.
pub fn ready(paths: &Paths, d: &DittoCatalog) -> bool {
    source_dir(paths, d).join(".venv").join("Scripts").join("python.exe").is_file() && d.files.iter().all(|f| models_dir(paths).join(&f.file).is_file())
}

/// How many bytes setting it up downloads, roughly (the packages' share is
/// an estimate, and nothing when the music trainer has them already).
pub fn setup_bytes(d: &DittoCatalog) -> u64 {
    d.uv.size + d.source.size + d.files.iter().map(|f| f.size).sum::<u64>() + 3_000_000_000
}

/// uv's arguments for one run: the voice and picture in, a silent video out.
pub fn args(source: &Path, models: &Path, image: &Path, audio: &Path, out: &Path, steps: u32) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["run".into(), "--frozen".into(), "--no-sync".into(), "--project".into(), source.into(), "python".into(), source.join("talk.py").into()];
    a.extend(["--models".into(), models.join(MODELS).into(), "--cfg".into(), models.join(CFG).into()]);
    a.extend(["--image".into(), image.into(), "--audio".into(), audio.into(), "--out".into(), out.into()]);
    a.extend(["--steps".into(), steps.clamp(10, 50).to_string().into()]);
    a
}

/// The environment it runs in: the music trainer's (uv's Python, packages
/// and cache, nothing fetched while it runs), with its progress bars quiet.
pub fn env(paths: &Paths) -> Vec<(&'static str, OsString)> {
    let mut e = super::train::trainer_env(paths);
    e.push(("TQDM_DISABLE", "1".into()));
    e
}

/// What a line of its output says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Said {
    Loaded,
    /// How many frames the video will have.
    Frames(u32),
    /// Frames written so far.
    Frame(u32),
    NoFace,
}

pub fn said(line: &str) -> Option<Said> {
    let rest = line.trim().strip_prefix("SULCUS ")?;
    let (word, n) = rest.split_once(' ').unwrap_or((rest, ""));
    match word {
        "loaded" => Some(Said::Loaded),
        "frames" => n.trim().parse().ok().map(Said::Frames),
        "frame" => n.trim().parse().ok().map(Said::Frame),
        "noface" => Some(Said::NoFace),
        _ => None,
    }
}

/// About how long a run takes: setting up the first time, loading, then
/// about four seconds of work for every second of voice on a 12 GB card.
pub fn estimate(seconds: f64, set_up: bool) -> f64 {
    (if set_up { 0.0 } else { 600.0 }) + 30.0 + 3.5 * seconds.clamp(0.5, LONGEST)
}

/// The picture's size for it: as it is, with its longest side at most
/// 1920 and both sides even (the video's frames need even sides).
pub fn picture_size(w: u32, h: u32) -> (u32, u32) {
    let k = (1920.0 / w.max(h).max(1) as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) / 2 * 2).max(64);
    (even(w as f64 * k), even(h as f64 * k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_lines_say_how_far_it_is() {
        assert_eq!(said("SULCUS loaded 7.1"), Some(Said::Loaded));
        assert_eq!(said("SULCUS frames 394"), Some(Said::Frames(394)));
        assert_eq!(said("SULCUS frame 120\r"), Some(Said::Frame(120)));
        assert_eq!(said("SULCUS noface"), Some(Said::NoFace));
        assert_eq!(said("writer: 12it [00:22, 5.86s/it]"), None);
        assert_eq!(said("SULCUS done 72.5"), None);
    }

    #[test]
    fn pictures_keep_their_shape_with_even_sides() {
        assert_eq!(picture_size(1432, 1432), (1432, 1432));
        assert_eq!(picture_size(4000, 3000), (1920, 1440));
        assert_eq!(picture_size(1023, 767), (1022, 766));
    }

    #[test]
    fn it_runs_from_its_folder_with_its_files() {
        let (e, m, w) = (Path::new("e").join("ditto-c3e47ee"), Path::new("m").join("ditto"), Path::new("w"));
        let a = args(&e, &m, &w.join("in.png"), &w.join("v.wav"), &w.join("out.mp4"), 80);
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(&a[..4], ["run", "--frozen", "--no-sync", "--project"]);
        assert!(a.contains(&e.join("talk.py").to_string_lossy().into_owned()));
        assert!(a.iter().any(|s| s.starts_with(&*m.to_string_lossy()) && s.ends_with("v0.4_hubert_cfg_pytorch.pkl")));
        assert_eq!(a.last().map(String::as_str), Some("50"), "steps are kept to 50 at most");
    }

    #[test]
    fn estimates_grow_with_the_voice() {
        assert!(estimate(10.0, true) < estimate(60.0, true));
        assert_eq!(estimate(10.0, false) - estimate(10.0, true), 600.0);
    }
}

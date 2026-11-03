// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures, video, music and narration, made on this PC.
//!
//! stable-diffusion.cpp makes and edits pictures, upscales them and makes
//! video clips; acestep.cpp makes music; BiRefNet on ONNX Runtime removes
//! backgrounds; narration uses the app's own voices. Each job runs the
//! engine program once, within the performance limits, one job at a time,
//! and what it makes goes into the gallery, encrypted.

pub(crate) mod artist;
pub mod clarify;
pub(crate) mod compose;
pub(crate) mod persona;
pub(crate) mod library;
pub mod coach;
mod cutout;
pub(crate) mod stems;
mod dereverb;
pub(crate) mod listen;
pub(crate) mod notes;
pub(crate) mod songmap;
pub(crate) mod imaging;
mod cloud;
pub mod hub;
mod ffmpeg;
pub mod loras;
pub mod learn;
pub mod look;
pub(crate) mod lyrics;
pub(crate) mod audio_in;
pub(crate) mod music;
pub(crate) mod yue;
pub(crate) mod heartmula;
pub(crate) mod train;
pub(crate) mod ditto;
pub(crate) mod studio;
pub mod own_voice;
pub mod podcast;
pub mod explainer;
pub(crate) mod proc;
pub(crate) mod svc;
pub(crate) mod transpose;
pub mod live_voice;
pub(crate) mod picture_train;
pub mod prompts;
pub mod screen;
mod sd;
pub mod store;
pub mod wallpaper;
mod longclip;
#[cfg(windows)]
mod mp4;
pub(crate) mod songvideo;

mod spec;
mod edits;
mod jobs;
mod checks;
mod pictures;
mod videos;
mod songs;
mod parts;
mod song_jobs;
mod commands;
#[cfg(test)]
mod tests;
pub use spec::*;
pub use edits::*;
pub use jobs::*;
use checks::*;
use pictures::*;
use videos::*;
pub(crate) use videos::camera_words;
use songs::*;
pub use parts::*;
pub use song_jobs::*;
pub use commands::*;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use base64::Engine as _;
use image::GenericImageView;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::catalog::{Budget, EngineSpec, License, GIB};
use crate::features::Feature;
use crate::hardware::Hardware;
use crate::paths::Paths;
use crate::{db, download, engine, net, AppState, AppStateRef};
pub use store::MediaItem;

const SD_EXE: &str = if cfg!(windows) { "sd-cli.exe" } else { "sd-cli" };
const MUSIC_EXE: &str = if cfg!(windows) { "ace-synth.exe" } else { "ace-synth" };
/// The reference PC's graphics memory bandwidth guess (the 12 GB tier in
/// `catalog`), for scaling the catalog's timings to this PC.
const REF_GPU_BW: f64 = 380.0;
/// The reference PC's processor cores (Core Ultra 9 275HX).
const REF_CORES: f64 = 24.0;
/// How much slower picture models run on the processor than on its card.
const CPU_SLOWDOWN: f64 = 14.0;
/// The music engine on the processor, against the graphics card.
const MUSIC_CPU_SLOWDOWN: f64 = 7.0;

// ---------- catalog ----------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaCatalog {
    pub engine: EngineSpec,
    pub music_engine: EngineSpec,
    pub models: Vec<MediaModel>,
    /// HTDemucs, which takes a song's parts apart for the song view (`stems`).
    #[serde(default)]
    pub separator: Option<Helper>,
    /// RealVisXL V5 as trained (not its few-step Lightning copy, whose
    /// LoRAs trained on it came out speckled): picture LoRAs learn on it.
    #[serde(default)]
    pub picture_trainer_base: Option<Helper>,
    /// FFmpeg, for editing videos.
    #[serde(default)]
    pub ffmpeg: Option<EngineSpec>,
    /// anvuew's de-reverb Mel-Band RoFormer, which takes a sound's reverb
    /// and echo out (the Studio's Remove FX, `dereverb`).
    #[serde(default)]
    pub dereverb: Option<Helper>,
    /// Basic Pitch, which reads the notes of a song's parts (`notes`).
    #[serde(default)]
    pub note_reader: Option<Helper>,
    /// The sampled instruments Compose plays its notes on (a General MIDI
    /// sound set, played in the window).
    #[serde(default)]
    pub instruments: Option<Helper>,
    /// yue2.cpp, the engine of the YuE2 music model (`yue`).
    #[serde(default)]
    pub yue_engine: Option<EngineSpec>,
    /// audio.cpp, the engine of the HeartMuLa music model (`heartmula`).
    #[serde(default)]
    pub heartmula_engine: Option<EngineSpec>,
    /// ACE-Step's trainer, for custom models (`train`).
    #[serde(default)]
    pub trainer: Option<train::TrainerCatalog>,
    /// Ditto, for quick talking pictures (`talk`).
    #[serde(default)]
    pub ditto: Option<ditto::DittoCatalog>,
    /// Narration and songs in the person's own recorded voice (`own_voice`).
    #[serde(default)]
    pub own_voice: Option<own_voice::OwnVoiceCatalog>,
}

/// A small model an engine's work uses along the way, fetched with the
/// first model that needs it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Helper {
    pub name: String,
    pub source: String,
    pub license: crate::catalog::License,
    pub file: MediaFile,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Image,
    Video,
    Music,
    Upscale,
    Background,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaFile {
    pub role: String,
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Defaults {
    pub steps: u32,
    pub cfg: f32,
    pub sampler: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub seconds: f32,
    pub flow_shift: f32,
    pub negative: String,
    /// The noise schedule ("karras"), when the model wants one.
    pub scheduler: String,
    /// Exact noise levels, comma-separated (distilled models tuned to them).
    pub sigmas: String,
    /// Extra settings the engine takes for some models ("key=value,…").
    pub model_args: String,
    /// Sides must be a multiple of this (16 when not set).
    pub align: u32,
    /// Video: the most frames the model was trained on (0: 81).
    pub max_frames: u32,
    /// Video: the engine's step cache ("easycache"), which skips work
    /// between similar steps.
    pub cache: String,
    /// Video: decode the frames in overlapping groups (less memory).
    pub temporal_tiling: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaModel {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub source: String,
    pub description: String,
    pub license: License,
    pub kind: Kind,
    #[serde(default)]
    pub can: Vec<String>,
    pub files: Vec<MediaFile>,
    pub min_vram_gb: f64,
    pub cpu: bool,
    pub secs: f64,
    pub quality: u8,
    /// What it's good at, and less good at, for choosing between models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strengths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weaknesses: Vec<String>,
    #[serde(default)]
    pub defaults: Defaults,
    /// Models on Hugging Face built on this one, offered by the Studio's search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<hub::HubFamily>,
    /// Installed from Hugging Face: its repository there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_hub: Option<String>,
    /// Installed from Hugging Face: the catalog model it's built on, so its
    /// prompts are written the way that family reads them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_on: Option<String>,
    /// A picture engine build of its own, when the shared one can't run it
    /// (Wan 2.2 S2V's 4-step LoRA crashed the shared build).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineSpec>,
}

/// Every media model: the catalog's, then those installed from Hugging Face.
pub fn models(state: &AppState) -> Vec<MediaModel> {
    let mut all = state.catalog.media.models.clone();
    all.extend(hub::list(&state.db.lock().unwrap()));
    all
}

impl MediaModel {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn can(&self, op: &str) -> bool {
        self.can.iter().any(|c| c == op)
    }

    fn file(&self, role: &str) -> Option<&MediaFile> {
        self.files.iter().find(|f| f.role == role)
    }
}

/// Which feature a kind of model belongs to.
pub fn feature_of(kind: Kind) -> Feature {
    match kind {
        Kind::Image | Kind::Upscale | Kind::Background => Feature::Images,
        Kind::Video => Feature::Video,
        Kind::Music => Feature::Music,
    }
}

fn op_kind(op: &str) -> Option<Kind> {
    Some(match op {
        "generate" | "edit" | "fill" | "extend" | "restyle" | "tune" | "coach" | "cover_art" => Kind::Image,
        "upscale" => Kind::Upscale,
        "remove_background" => Kind::Background,
        "video" | "speak" | "talk" => Kind::Video,
        "music" | "sound" | "parts" | "remake" => Kind::Music,
        _ => return None,
    })
}

/// What a job asks of its model: a song's parts (adding, taking out or
/// completing one) need ACE-Step Base; everything else is the job's own kind.
fn model_op(req: &Request) -> &str {
    match req.music.as_ref().and_then(|m| m.task.as_deref()) {
        Some("add" | "extract" | "complete") if req.op == "music" => "parts",
        // Making unclear parts again is music made again, by a music model.
        _ if req.op == "remake" => "music",
        // A Studio part is added, then taken out alone: ACE-Step Base's work.
        _ if req.op == "studio_part" => "parts",
        // A song's cover art is a new picture.
        _ if req.op == "cover_art" => "generate",
        _ => &req.op,
    }
}

fn op_feature(op: &str) -> Option<Feature> {
    match op {
        "narrate" | "podcast" | "dialogue" | "listen" | "map" | "revoice" | "stems" | "dry" | "song_video" | "train" | "trainer" | "studio_part" => Some(Feature::Music),
        "video_edit" | "video_sound" | "dub" | "captions" => Some(Feature::Video),
        "train_picture" => Some(Feature::Images),
        // Study material.
        "explainer" => Some(Feature::Documents),
        _ => op_kind(op).map(feature_of),
    }
}

// ---------- fit ----------

#[derive(Debug, Clone, Serialize)]
pub struct MediaFit {
    pub runnable: bool,
    /// Runs on the graphics card (else the processor).
    pub on_gpu: bool,
    /// Seconds for one default job on this PC (a rough guide).
    pub est_secs: f64,
    /// Bytes still to download (files other models share are counted once).
    pub needed_bytes: u64,
    pub disk_ok: bool,
    /// Why it can't run here.
    pub reason: Option<String>,
}

fn cores(hw: &Hardware) -> f64 {
    hw.cpu_cores.unwrap_or(hw.cpu_threads / 2).max(1) as f64
}

pub fn fit(m: &MediaModel, hw: &Hardware, b: &Budget, missing: u64, measured: Option<f64>) -> MediaFit {
    let cpu_scale = (REF_CORES / cores(hw)).powf(0.8) * if hw.avx2 { 1.0 } else { 2.5 };
    // Picture, video and music engines run on one card: the biggest.
    let gpu_ok = b.vram_card > 0 && b.vram_card as f64 >= m.min_vram_gb * GIB as f64;
    // Weights that don't fit on the card are streamed from memory.
    let mem_ok = m.size() <= b.vram_card + b.ram;
    let (runnable, on_gpu, est, reason) = if !mem_ok {
        (false, false, 0.0, Some(format!("needs about {:.0} GB of memory", m.size() as f64 / GIB as f64 + 1.0)))
    } else {
        match m.kind {
            // Music runs on the graphics card when there is one (the Vulkan
            // engine); its seconds are measured on an RTX 5070-class card.
            Kind::Music if b.vram_card > 0 => (true, true, m.secs * REF_GPU_BW / b.gpu_bw_card.max(50.0), None),
            Kind::Music => (true, false, m.secs * MUSIC_CPU_SLOWDOWN * cpu_scale, None),
            // This engine runs on the processor.
            Kind::Background => (true, false, m.secs * cpu_scale, None),
            _ if gpu_ok => (true, true, m.secs * REF_GPU_BW / b.gpu_bw_card.max(50.0), None),
            _ if m.cpu => (true, false, m.secs * CPU_SLOWDOWN * cpu_scale, None),
            _ => {
                let have = b.vram_card as f64 / GIB as f64;
                let why = if b.vram_card == 0 {
                    format!("needs a graphics card with at least {:.0} GB of its own memory, and this PC has none", m.min_vram_gb)
                } else {
                    format!("needs a graphics card with at least {:.0} GB; this PC's has about {have:.0} GB free for it", m.min_vram_gb)
                };
                (false, false, 0.0, Some(why))
            }
        }
    };
    MediaFit {
        runnable,
        on_gpu,
        est_secs: measured.filter(|_| runnable).unwrap_or(est).round(),
        needed_bytes: missing,
        disk_ok: missing <= b.disk,
        reason,
    }
}

// ---------- installed models ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledMedia {
    pub model_id: String,
    pub installed_at: i64,
    /// Measured seconds for one default job, once it has run.
    #[serde(default)]
    pub secs: Option<f64>,
    /// The same for a checked edit (planned, made, checked, maybe made again).
    #[serde(default)]
    pub edit_secs: Option<f64>,
}

const INSTALLED_KEY: &str = "media_installed";
const DEFAULTS_KEY: &str = "media_defaults";

/// The model the person picked to use unless a job names one (chats and
/// the Studio), per kind.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaDefaults {
    pub image: Option<String>,
    pub video: Option<String>,
    pub music: Option<String>,
}

pub fn defaults(conn: &Connection) -> MediaDefaults {
    db::get(conn, DEFAULTS_KEY).unwrap_or_default()
}

/// Sets (or with None clears) the default model for "image", "video" or "music".
#[tauri::command(async)]
pub fn media_set_default(state: AppStateRef, kind: String, model_id: Option<String>) -> Result<MediaDefaults, String> {
    let conn = state.db.lock().unwrap();
    let mut d = defaults(&conn);
    let slot = match kind.as_str() {
        "image" => &mut d.image,
        "video" => &mut d.video,
        "music" => &mut d.music,
        _ => return Err("Unknown kind of model.".into()),
    };
    *slot = model_id.filter(|m| !m.is_empty());
    db::set(&conn, DEFAULTS_KEY, &d)?;
    Ok(d)
}

pub fn installed(conn: &Connection) -> Vec<InstalledMedia> {
    db::get(conn, INSTALLED_KEY).unwrap_or_default()
}

fn save_installed(conn: &Connection, list: &[InstalledMedia]) -> Result<(), String> {
    db::set(conn, INSTALLED_KEY, &list)
}

/// Takes media models whose files are gone off the list (after a restore on
/// another PC). Returns how many.
pub fn prune_missing(conn: &Connection, paths: &Paths, c: &MediaCatalog) -> usize {
    let mut list = installed(conn);
    let before = list.len();
    let from_hub = hub::list(conn);
    list.retain(|i| c.models.iter().chain(&from_hub).find(|m| m.id == i.model_id).is_some_and(|m| missing_bytes(paths, m) == 0));
    let gone = before - list.len();
    if gone > 0 {
        let _ = save_installed(conn, &list);
    }
    gone
}

/// Any model of this kind installed.
pub fn has_kind(state: &AppState, kind: Kind) -> bool {
    let list = installed(&state.db.lock().unwrap());
    models(state).iter().any(|m| m.kind == kind && list.iter().any(|i| i.model_id == m.id))
}

pub fn model_dir(paths: &Paths, m: &MediaModel) -> PathBuf {
    let dir = paths.models.join("media");
    if m.kind == Kind::Music {
        dir.join("music")
    } else {
        dir
    }
}

fn file_path(paths: &Paths, m: &MediaModel, f: &MediaFile) -> PathBuf {
    model_dir(paths, m).join(&f.file)
}

/// A file of an installed media model, by its role ("llm", "llm_vision"),
/// when it's whole on disk.
pub fn installed_file(state: &AppState, m: &MediaModel, role: &str) -> Option<PathBuf> {
    let f = m.file(role)?;
    if !installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == m.id) {
        return None;
    }
    let p = file_path(&state.paths, m, f);
    p.metadata().is_ok_and(|md| md.len() == f.size).then_some(p)
}

fn missing_bytes(paths: &Paths, m: &MediaModel) -> u64 {
    m.files.iter().filter(|f| !file_path(paths, m, f).metadata().is_ok_and(|md| md.len() == f.size)).map(|f| f.size).sum()
}

/// An installed model whose files have gone (removed by hand, or a download
/// that never finished) stops the job with what to do, not the engine's
/// "not found in registry"; and, with the models folder there (not a drive
/// that's away), it's no longer listed as installed so it can be installed
/// again.
fn files_here(state: &AppState, m: &MediaModel) -> Result<(), String> {
    let gone: Vec<&str> = m.files.iter().filter(|f| !file_path(&state.paths, m, f).metadata().is_ok_and(|md| md.len() == f.size)).map(|f| f.file.as_str()).collect();
    if gone.is_empty() {
        return Ok(());
    }
    let dir = model_dir(&state.paths, m);
    crate::applog::line("media", &format!("{}'s files are missing from {}: {}", m.name, dir.display(), gone.join(", ")));
    if state.paths.models.is_dir() {
        let conn = state.db.lock().unwrap();
        let mut list = installed(&conn);
        list.retain(|i| i.model_id != m.id);
        save_installed(&conn, &list)?;
    }
    Err(format!("{}'s model files are missing from {} (removed, or a download that didn't finish). Install it again from Models in the Studio.", m.name, dir.display()))
}

/// Files an update added to a model that's already installed (S2V's 4-step
/// LoRA) come down when it's next used, instead of it looking broken. Only
/// small additions: a big file that's gone is left to "Install it again".
async fn fetch_added(state: &Arc<AppState>, m: &MediaModel, cancel: &AtomicBool) -> Result<(), String> {
    let gone: u64 = m.files.iter().filter(|f| !file_path(&state.paths, m, f).is_file()).map(|f| f.size).sum();
    let here = installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == m.id);
    if gone == 0 || gone > 1_500_000_000 || !here || !state.paths.models.is_dir() {
        return Ok(());
    }
    crate::applog::line("media", &format!("{}: getting the parts an update added", m.name));
    install(state, m, cancel, &|_, _, _| {}).await
}

fn sd_dir(engine: &EngineSpec, backend: &str) -> String {
    format!("sd-{}-{backend}", engine.build)
}

/// The picture engine a model runs on: its own build, else the shared one.
fn engine_of<'a>(c: &'a MediaCatalog, m: &'a MediaModel) -> &'a EngineSpec {
    m.engine.as_ref().unwrap_or(&c.engine)
}

/// A model's own picture engine, downloaded the first time: the build for
/// this PC's card, or the processor's when the job was moved there.
async fn own_engine(state: &AppState, e: &EngineSpec, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let (key, asset) = if ON_PROCESSOR.try_with(|p| *p).unwrap_or(false) {
        [engine::cpu_key(), "cpu-x64", "linux-cpu-x64"].into_iter().find_map(|k| e.assets.get(k).map(|a| (k, a)))
    } else {
        let b = engine::backend_for(&state.budget())?;
        e.assets.get(b).map(|a| (b, a))
    }
    .ok_or("There's no build of this model's picture engine for this PC.")?;
    let dir = sd_dir(e, key);
    if engine::find_exe(&state.paths.engines.join(&dir), SD_EXE).is_none() {
        state.log("network", &format!("Downloading the stable-diffusion.cpp engine ({key}) from {}", crate::host_of(&asset.url)));
    }
    engine::ensure_unpacked(&state.paths, &dir, asset, SD_EXE, cancel, |_, _| {}).await
}

fn music_dir(c: &MediaCatalog, key: &str) -> String {
    format!("acestep-{}-{key}", c.music_engine.build)
}

/// The installed music engine: the graphics-card build if it's here, else
/// the processor one.
fn yue_dir(e: &EngineSpec, key: &str) -> String {
    format!("yue2-{}-{key}", e.build)
}

/// The installed YuE2 engine (the graphics card's build first).
pub(crate) fn yue_engine_dir(state: &AppState) -> Option<PathBuf> {
    let e = state.catalog.media.yue_engine.as_ref()?;
    ["vulkan-x64", engine::cpu_key()].iter().map(|k| state.paths.engines.join(yue_dir(e, k))).find(|d| engine::find_exe(d, yue::EXE).is_some())
}

fn heartmula_dir(e: &EngineSpec) -> String {
    format!("audiocpp-{}-vulkan-x64", e.build)
}

/// The installed HeartMuLa engine (audio.cpp's Vulkan build).
pub(crate) fn heartmula_engine_dir(state: &AppState) -> Option<PathBuf> {
    let e = state.catalog.media.heartmula_engine.as_ref()?;
    Some(state.paths.engines.join(heartmula_dir(e))).filter(|d| engine::find_exe(d, heartmula::EXE).is_some())
}

pub(crate) fn music_engine_dir(state: &AppState) -> Option<PathBuf> {
    let c = &state.catalog.media;
    ["vulkan-x64", engine::cpu_key()]
        .iter()
        .map(|k| state.paths.engines.join(music_dir(c, k)))
        .find(|d| engine::find_exe(d, MUSIC_EXE).is_some())
}

fn sd_exe(state: &AppState) -> Result<PathBuf, String> {
    let backend = engine::backend_for(&state.budget())?;
    engine::find_exe(&state.paths.engines.join(sd_dir(&state.catalog.media.engine, backend)), SD_EXE)
        .ok_or_else(|| "The picture engine isn't installed. Reinstall the model from the Studio's Models list.".to_string())
}

/// The installed model to use for a job: the one asked for, else the best
/// installed one that can do it.
pub fn choose(state: &AppState, op: &str, wanted: Option<&str>, needs_image_input: bool) -> Result<MediaModel, String> {
    let kind = op_kind(op).ok_or("Unknown kind of job.")?;
    let list = installed(&state.db.lock().unwrap());
    let all = models(state);
    let ok = |m: &&MediaModel| {
        m.kind == kind
            && list.iter().any(|i| i.model_id == m.id)
            && (kind != Kind::Image || m.can(if matches!(op, "tune" | "coach") { "generate" } else { op }))
            && (kind != Kind::Music || m.can(op))
            && (!needs_image_input || kind != Kind::Video || op == "speak" || m.can("image"))
            // Lips to a voice (Wan 2.2 S2V) is its own job: that model needs a voice.
            && (kind != Kind::Video || m.can("speech") == (op == "speak"))
    };
    if let Some(id) = wanted.filter(|w| !w.is_empty()) {
        if let Some(m) = all.iter().find(|m| m.id == id).filter(ok) {
            return Ok(m.clone());
        }
    }
    // The person's default for this kind, when it can do the job. It wins
    // over "fits the card": the job frees the card (a loaded chat model)
    // before it starts, so the free memory now says little.
    let d = defaults(&state.db.lock().unwrap());
    let preferred = match kind {
        Kind::Image => d.image,
        Kind::Video => d.video,
        Kind::Music => d.music,
        _ => None,
    };
    if let Some(m) = preferred.and_then(|id| all.iter().find(|m| m.id == id).filter(ok)) {
        return Ok(m.clone());
    }
    // The best one that fits the graphics card, so the job stays on it.
    let fits = |m: &&MediaModel| {
        let b = state.budget();
        b.vram_card > 0 && b.vram_card as f64 >= m.min_vram_gb * GIB as f64
    };
    // Catalog models first among equals: they're the tested ones.
    let rank = |m: &&MediaModel| (m.quality, m.from_hub.is_none());
    if let Some(m) = all.iter().filter(ok).filter(fits).max_by_key(rank) {
        return Ok(m.clone());
    }
    if let Some(m) = all.iter().filter(ok).max_by_key(rank) {
        return Ok(m.clone());
    }
    Err(match (kind, op) {
        (Kind::Image, "edit" | "restyle") => "Edits from instructions need FLUX.2 klein. Install it from the Studio's Models list.".into(),
        (Kind::Image, _) => "Install a picture model first: open the Studio and pick one under Models.".into(),
        (Kind::Video, "speak") => "Making a picture talk or sing needs Wan 2.2 S2V. Install it from the Studio's Models list.".into(),
        (Kind::Video, _) if needs_image_input => "Bringing a picture to life needs Wan 2.2. Install it from the Studio's Models list.".into(),
        (Kind::Video, _) => "Install a video model first: open the Studio and pick one under Models.".into(),
        (Kind::Music, "parts") => "Adding a part to a song, taking one out or building around one needs ACE-Step 1.5 Base. Install it from the Studio's Models list.".into(),
        (Kind::Music, "sound") => "Sound effects need ACE-Step 1.5 or its small version. Install one from the Studio's Models list.".into(),
        (Kind::Music, _) => "Install a music model first: open the Studio and pick one under Models.".into(),
        (Kind::Upscale, _) => "Upscaling needs Real-ESRGAN. Install it from the Studio's Models list (64 MB).".into(),
        (Kind::Background, _) => "Removing backgrounds needs BiRefNet. Install it from the Studio's Models list (224 MB).".into(),
    })
}

// ---------- install ----------

pub(crate) async fn install(state: &Arc<AppState>, spec: &MediaModel, cancel: &AtomicBool, emit: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<(), String> {
    let c = &state.catalog.media;
    emit("engine", 0, 0);
    match spec.kind {
        // HeartMuLa runs on audio.cpp's Vulkan build: on a graphics card only.
        Kind::Music if spec.file("heartmula").is_some() => {
            let e = c.heartmula_engine.as_ref().ok_or("HeartMuLa isn't available on this system yet.")?;
            let asset = e.assets.get("vulkan-x64").filter(|_| cfg!(windows)).ok_or("HeartMuLa isn't available on this system yet.")?;
            if state.budget().vram == 0 {
                return Err("HeartMuLa needs a graphics card with 10 GB of memory or more.".into());
            }
            let dir = heartmula_dir(e);
            if engine::find_exe(&state.paths.engines.join(&dir), heartmula::EXE).is_none() {
                state.log("network", &format!("Downloading the audio.cpp music engine (graphics card) from {}", crate::host_of(&asset.url)));
            }
            engine::ensure_unpacked(&state.paths, &dir, asset, heartmula::EXE, cancel, |r, t| emit("engine", r, t)).await?;
        }
        // YuE2 has an engine of its own.
        Kind::Music if spec.file("yue").is_some() => {
            let e = c.yue_engine.as_ref().ok_or("YuE2 isn't available on this system yet.")?;
            let gpu = state.budget().vram > 0 && cfg!(windows);
            let mut done = false;
            if let Some(asset) = e.assets.get("vulkan-x64").filter(|_| gpu) {
                let dir = yue_dir(e, "vulkan-x64");
                if engine::find_exe(&state.paths.engines.join(&dir), yue::EXE).is_none() {
                    state.log("network", &format!("Downloading the yue2.cpp music engine (graphics card) from {}", crate::host_of(&asset.url)));
                }
                match engine::ensure_unpacked(&state.paths, &dir, asset, yue::EXE, cancel, |r, t| emit("engine", r, t)).await {
                    Ok(_) => done = true,
                    Err(e) if e == download::CANCELLED => return Err(e),
                    Err(e) => state.log("media", &format!("The graphics-card YuE2 engine didn't download ({e}); using the processor one")),
                }
            }
            if !done {
                let asset = e.assets.get(engine::cpu_key()).ok_or("YuE2 isn't available on this system yet.")?;
                let dir = yue_dir(e, engine::cpu_key());
                if engine::find_exe(&state.paths.engines.join(&dir), yue::EXE).is_none() {
                    state.log("network", &format!("Downloading the yue2.cpp music engine from {}", crate::host_of(&asset.url)));
                }
                engine::ensure_unpacked(&state.paths, &dir, asset, yue::EXE, cancel, |r, t| emit("engine", r, t)).await?;
            }
        }
        Kind::Music => {
            // The graphics-card build is about twice as fast; it also runs on
            // the processor, but it's a bigger download, so only with a card.
            let gpu = state.budget().vram > 0 && cfg!(windows);
            let mut done = false;
            if let Some(asset) = c.music_engine.assets.get("vulkan-x64").filter(|_| gpu) {
                let dir = music_dir(c, "vulkan-x64");
                if engine::find_exe(&state.paths.engines.join(&dir), MUSIC_EXE).is_none() {
                    state.log("network", &format!("Downloading the acestep.cpp music engine (graphics card) from {}", crate::host_of(&asset.url)));
                }
                match engine::ensure_unpacked(&state.paths, &dir, asset, MUSIC_EXE, cancel, |r, t| emit("engine", r, t)).await {
                    Ok(_) => done = true,
                    Err(e) if e == download::CANCELLED => return Err(e),
                    Err(e) => state.log("media", &format!("The graphics-card music engine didn't download ({e}); using the processor one")),
                }
            }
            if !done {
                let asset = c.music_engine.assets.get(engine::cpu_key()).ok_or("Music isn't available on this system yet.")?;
                let dir = music_dir(c, engine::cpu_key());
                if engine::find_exe(&state.paths.engines.join(&dir), MUSIC_EXE).is_none() {
                    state.log("network", &format!("Downloading the acestep.cpp music engine from {}", crate::host_of(&asset.url)));
                }
                engine::ensure_unpacked(&state.paths, &dir, asset, MUSIC_EXE, cancel, |r, t| emit("engine", r, t)).await?;
            }
        }
        Kind::Background => {
            crate::natural::ensure_runtime(state, cancel, emit).await?;
        }
        _ => {
            let backend = engine::backend_for(&state.budget())?;
            let engine = engine_of(c, spec);
            let asset = engine.assets.get(backend).ok_or("No picture engine for this PC")?;
            let dir = sd_dir(engine, backend);
            if engine::find_exe(&state.paths.engines.join(&dir), SD_EXE).is_none() {
                state.log("network", &format!("Downloading the stable-diffusion.cpp engine ({backend}) from {}", crate::host_of(&asset.url)));
            }
            engine::ensure_unpacked(&state.paths, &dir, asset, SD_EXE, cancel, |r, t| emit("engine", r, t)).await?;
        }
    }
    let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
    let total = spec.size();
    let mut done = 0u64;
    for f in &spec.files {
        let dest = file_path(&state.paths, spec, f);
        if !dest.exists() {
            state.log("network", &format!("Downloading {} ({}) from {}", f.file, crate::size_label(f.size), crate::host_of(&f.url)));
        }
        let base = done;
        download::fetch_verified(&client, &f.url, &dest, f.size, &f.sha256, cancel, |r, _| emit("download", base + r, total)).await?;
        done += f.size;
    }
    // A music model from Hugging Face: its own file must be one the music
    // engine runs (it skips others, and the song would fail later).
    if spec.kind == Kind::Music && spec.from_hub.is_some() {
        for f in spec.files.iter().filter(|f| f.file.starts_with("hfm-")) {
            let dest = file_path(&state.paths, spec, f);
            let arch = crate::gguf::read(&dest).ok().and_then(|m| m.get("general.architecture").and_then(|v| v.str().map(str::to_string)));
            if let Some(why) = hub::music_file_problem(&f.role, arch.as_deref()) {
                crate::applog::line("media", &format!("{} isn't a music engine file ({arch:?}); deleted it", f.file));
                std::fs::remove_file(&dest).ok();
                return Err(why.into());
            }
        }
    }
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    if !list.iter().any(|i| i.model_id == spec.id) {
        list.push(InstalledMedia { model_id: spec.id.clone(), installed_at: db::now_ms(), secs: None, edit_secs: None });
    }
    save_installed(&conn, &list)
}

/// Media models whose files are already on this PC join the list without a
/// download, once their engine is here too. Returns their names.
pub(crate) fn adopt_found(state: &AppState, finder: &mut crate::found::Finder) -> Vec<String> {
    use crate::found::Place;
    let c = &state.catalog.media;
    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let busy: Vec<String> = state.installs.lock().unwrap().keys().cloned().collect();
    let have = installed(&state.db.lock().unwrap());
    let mut names = Vec::new();
    for m in &c.models {
        if busy.contains(&m.id) || have.iter().any(|i| i.model_id == m.id) || !fit(m, &hw, &b, 0, None).runnable {
            continue;
        }
        let mut place = Place::Here;
        for f in &m.files {
            match finder.place(&file_path(&state.paths, m, f), f.size, &f.sha256) {
                Place::Missing => {
                    place = Place::Missing;
                    break;
                }
                Place::From(app) => place = Place::From(app),
                Place::Here => {}
            }
        }
        let engine_ready = match m.kind {
            Kind::Music => music_engine_dir(state).is_some(),
            Kind::Background => crate::natural::runtime_dll(&state.paths, &state.catalog.voices).is_some(),
            _ => engine::backend_for(&b).is_ok_and(|be| engine::find_exe(&state.paths.engines.join(sd_dir(engine_of(c, m), be)), SD_EXE).is_some()),
        };
        if place == Place::Missing || !engine_ready {
            continue;
        }
        {
            let conn = state.db.lock().unwrap();
            let mut list = installed(&conn);
            list.push(InstalledMedia { model_id: m.id.clone(), installed_at: db::now_ms(), secs: None, edit_secs: None });
            if save_installed(&conn, &list).is_err() {
                continue;
            }
        }
        state.log("model", &crate::found::note(&m.name, &place));
        names.push(m.name.clone());
    }
    names
}

#[cfg(test)]
pub async fn install_for_test(state: &Arc<AppState>, id: &str) -> Result<(), String> {
    let spec = models(state).into_iter().find(|m| m.id == id).ok_or("unknown")?;
    install(state, &spec, &AtomicBool::new(false), &|_, _, _| {}).await
}

/// Picks the model to offer when a media feature is turned on.
pub fn recommended(state: &AppState, kind: Kind) -> Option<MediaModel> {
    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let candidates: Vec<&MediaModel> = state.catalog.media.models.iter().filter(|m| m.kind == kind).collect();
    let runs = |m: &&&MediaModel| fit(m, &hw, &b, missing_bytes(&state.paths, m), None).runnable;
    // On the processor, the quickest; on a graphics card, the best that takes under a minute.
    let ok: Vec<&&MediaModel> = candidates.iter().filter(runs).collect();
    let quick = ok.iter().filter(|m| fit(m, &hw, &b, 0, None).est_secs <= if kind == Kind::Video { 600.0 } else { 60.0 }).max_by_key(|m| m.quality);
    quick.or_else(|| ok.iter().min_by(|a, b| a.secs.total_cmp(&b.secs))).map(|m| (**m).clone())
}

// ---------- jobs ----------

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Request {
    /// generate, edit, fill, extend, restyle, upscale, remove_background,
    /// video, music, sound or narrate.
    pub op: String,
    pub model_id: Option<String>,
    /// The description, instruction, style, or text to read.
    pub prompt: String,
    pub negative: Option<String>,
    /// The gallery item to work from.
    pub source: Option<String>,
    /// PNG, base64: where to change (filling in).
    pub mask: Option<String>,
    /// square, portrait, landscape, wide or tall.
    pub shape: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub count: Option<u32>,
    pub seed: Option<i64>,
    pub steps: Option<u32>,
    /// Pixels to add: left, top, right, bottom (extending).
    pub extend: Option<[u32; 4]>,
    pub seconds: Option<f32>,
    pub lyrics: Option<String>,
    pub instrumental: Option<bool>,
    /// A song made from the song view: map the new version too (`map_item`).
    pub then_map: Option<bool>,
    /// A custom model to train on the person's songs (op "train").
    pub train: Option<train::TrainSpec>,
    /// A picture LoRA to train on the person's photos (op "train_picture").
    pub picture_train: Option<picture_train::PictureTrainSpec>,
    /// Music: how the song is shaped, and work on a song the person has.
    pub music: Option<MusicSpec>,
    pub voice: Option<String>,
    pub language: Option<String>,
    pub chat_id: Option<String>,
    /// A podcast or a reading with several voices: who talks, and what to
    /// write about.
    pub talk: Option<podcast::Spec>,
    /// How a new picture looks (see `STYLES`); none or "auto": a realistic
    /// photo, unless the description itself names a look.
    pub style: Option<String>,
    /// Example pictures (gallery ids) the new one should follow.
    pub examples: Vec<String>,
    /// What to take from them: "style", "subject" or "layout".
    pub example_use: Option<String>,
    /// The person's own LoRAs for this picture, with how strongly each is used.
    pub loras: Vec<(String, f32)>,
    /// A picture (gallery id) whose people's pose the new one takes.
    pub pose: Option<String>,
    /// A video's camera move (`videos::CAMERA`), added to its description.
    pub camera: Option<String>,
    /// The picture (gallery id) a video ends on; it starts on `source`.
    pub end: Option<String>,
    /// What to do to a video (`video_edit`).
    pub edit: Option<VideoEdit>,
    /// What a picture says or sings (`speak`): a song or sound from the
    /// gallery, a sound file, or else `say` read aloud in `voice`.
    pub sound: Option<String>,
    pub sound_file: Option<String>,
    pub say: Option<String>,
    /// The prompt writer turns the description into the one this model
    /// follows best (None: only for models that need a special form, or
    /// with example pictures).
    pub write_prompt: Option<bool>,
    /// The picture reader checks the picture against what was asked for and,
    /// when things are missing, it's made once more with a better description.
    pub check: Option<bool>,
    /// What else the description must serve ("desktop wallpaper…").
    pub notes: Option<String>,
    /// A desktop wallpaper: made for this screen size (a still picture with
    /// "generate", a looping clip with "video").
    pub wallpaper: Option<wallpaper::Spec>,
    /// The person's own words, when the prompt writer replaced the prompt
    /// (kept with what's made).
    #[serde(skip)]
    pub asked: Option<String>,
    /// Songs: why lyrics couldn't be written before the job (the music
    /// model then writes its own).
    #[serde(skip)]
    pub lyrics_problem: Option<lyrics::NotWritten>,
}

/// Looks a new picture can have: id and the words added to its description.
pub const STYLES: &[(&str, &str)] = &[
    ("photo", "realistic photograph, natural lighting, true-to-life colors and skin texture, fine detail, sharp focus, shot on a full-frame camera"),
    // A snapshot from someone's phone: what makes a picture pass for real.
    // Naming the phone puts a hand holding one in the picture, so it's described by its look instead.
    ("phone_photo", "candid amateur snapshot of the whole scene, real everyday place, natural available light, true-to-life colors and textures, unedited, slightly imperfect framing, shot from eye level"),
    ("cinematic", "cinematic film still, realistic, dramatic lighting, shallow depth of field, film grain"),
    ("cartoon", "cartoon illustration, bold outlines, flat bright colors"),
    ("anime", "anime style, cel shading, clean line art"),
    ("digital_art", "digital art, detailed digital painting, vibrant colors"),
    ("oil_painting", "oil painting on canvas, visible brush strokes, rich colors"),
    ("watercolor", "watercolor painting, soft washes of color, paper texture"),
    ("sketch", "pencil sketch, graphite shading, hand-drawn on paper"),
    ("3d", "3D render, soft studio lighting, smooth materials"),
    ("pixel_art", "pixel art, 16-bit video game style, crisp pixels"),
];

/// Words in a description that already ask for a look other than a photo.
const LOOK_WORDS: &[&str] = &[
    "cartoon", "anime", "manga", "illustration", "illustrated", "drawing", "drawn", "sketch", "painting", "painted", "watercolor", "watercolour",
    "oil paint", "3d", "render", "pixel", "comic", "clipart", "clip art", "vector", "logo", "icon", "chibi", "low poly", "claymation", "storybook",
    "pastel", "digital art", "concept art", "photo", "photograph", "realistic", "cinematic", "film still", "snapshot", "selfie",
];

/// The description with its style's words added. Auto adds the photo look
/// unless the description already names one.
pub fn styled(prompt: &str, style: Option<&str>) -> String {
    let prompt = prompt.trim();
    let style = style.unwrap_or("auto");
    let id = if style == "auto" {
        let low = prompt.to_lowercase();
        if LOOK_WORDS.iter().any(|w| has_word(&low, w)) {
            return prompt.to_string();
        }
        "photo"
    } else {
        style
    };
    match STYLES.iter().find(|(s, _)| *s == id) {
        Some((_, words)) => format!("{}, {words}", prompt.trim_end_matches(['.', ','])),
        None => prompt.to_string(),
    }
}

/// The installed picture model best at writing words into pictures
/// (Qwen-Image), for a description with text in quotes.
pub fn best_for_text(state: &AppState, prompt: &str) -> Option<String> {
    let quoted = prompt.split(['"', '\u{201c}', '\u{201d}']).skip(1).step_by(2).any(|q| q.trim().chars().count() >= 2);
    if !quoted {
        return None;
    }
    let list = installed(&state.db.lock().unwrap());
    models(state)
        .into_iter()
        .find(|m| m.kind == Kind::Image && m.can("generate") && prompts::family(m) == "qwen-image" && list.iter().any(|i| i.model_id == m.id))
        .map(|m| m.id)
}

/// An edit that changes how people look rather than what's around them.
fn changes_how_people_look(instruction: &str) -> bool {
    let low = instruction.to_lowercase();
    [
        "older", "younger", "age ", "aged", "years old", "hair", "bald", "beard", "mustache", "moustache", "makeup", "make-up", "face", "smile", "skin", "tattoo",
        "cartoon", "anime", "painting", "drawing", "sketch", "style", "pixar", "disney", "3d", "zombie", "elf", "superhero", "turn me into", "turn him into",
        "turn her into", "turn them into", "turn us into", "make me look", "make him look", "make her look", "make them look", "as a ",
    ]
    .iter()
    .any(|w| low.contains(w))
}

/// `word` in `text` as a whole word (or its plural): "icon" isn't in
/// "iconic", "photo" isn't in "photographer".
fn has_word(text: &str, word: &str) -> bool {
    let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let at = from + i;
        let end = at + word.len();
        let before = text[..at].chars().next_back();
        let mut after = text[end..].chars();
        let next = after.next();
        let ok_end = !alnum(next) || (next == Some('s') && !alnum(after.next()));
        if !alnum(before) && ok_end {
            return true;
        }
        from = end;
    }
    false
}

/// Looks that aren't photographs.
const DRAWN_LOOKS: &[&str] = &["cartoon", "anime", "digital_art", "oil_painting", "watercolor", "sketch", "3d", "pixel_art"];
const DRAWN_WORDS: &[&str] = &[
    "cartoon", "anime", "manga", "illustration", "drawing", "drawn", "sketch", "painting", "painted", "watercolor", "watercolour", "oil paint", "3d",
    "render", "pixel art", "comic", "vector", "chibi", "low poly", "claymation", "digital art", "concept art", "cel shading", "storybook",
];

/// The negative prompt without the words that would undo the chosen look
/// (an SDXL negative lists "cartoon, painting, 3d" to keep photos real;
/// for a cartoon, those have to go).
pub fn fit_negative(negative: &str, look: Option<&str>, prompt: &str) -> String {
    let low = prompt.to_lowercase();
    let drawn = look.is_some_and(|l| DRAWN_LOOKS.contains(&l)) || DRAWN_WORDS.iter().any(|w| has_word(&low, w));
    if !drawn {
        return negative.to_string();
    }
    const UNDOES: &[&str] = &["illustration", "3d", "2d", "painting", "cartoon", "sketch", "drawing", "anime", "render", "cgi", "semi-realistic", "digital art", "comic"];
    negative
        .split(',')
        .map(|t| t.trim().trim_matches(|c| c == '(' || c == ')').trim())
        .filter(|t| !t.is_empty() && !UNDOES.iter().any(|u| has_word(&t.to_lowercase(), u)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub id: String,
    pub op: String,
    pub prompt: String,
    pub model_id: Option<String>,
    /// "queued" or "running".
    pub status: String,
    pub stage: String,
    pub fraction: f64,
    pub eta_secs: Option<f64>,
    pub est_secs: f64,
    pub chat_id: Option<String>,
    /// What's making it: "graphics card", "processor" or "cloud" (while it runs).
    pub on: Option<String>,
    /// Which one: the card's or processor's name, or the cloud model.
    pub device: Option<String>,
    /// The graphics card now: percent busy and °C.
    pub gpu_usage: Option<f32>,
    pub gpu_temp: Option<f32>,
    /// The processor now: percent busy.
    pub cpu_usage: Option<f32>,
    /// Memory in use now, percent: the PC's, and the graphics card's.
    #[serde(default)]
    pub ram_usage: Option<f32>,
    #[serde(default)]
    pub gpu_mem: Option<f32>,
}

struct Job {
    info: JobInfo,
    cancel: Arc<AtomicBool>,
}

fn jobs() -> &'static Mutex<Vec<Job>> {
    static J: OnceLock<Mutex<Vec<Job>>> = OnceLock::new();
    J.get_or_init(Default::default)
}

/// A picture, video or music job is queued or running.
pub fn any_job() -> bool {
    !jobs().lock().unwrap().is_empty()
}

tokio::task_local! {
    /// The job the work belongs to (for questions asked on its behalf).
    static JOB_ID: String;
    /// The job runs on the second card (the main one is busy).
    static SECOND_CARD: bool;
    /// Started now on the processor, as the person asked (the card was busy).
    static ON_PROCESSOR: bool;
    /// The picture engine for this job's model, when it has a build of its own.
    static SD_ENGINE: PathBuf;
}

/// The graphics memory a job needs, for its place in line: its model's on
/// the card, the most a song's parts or a sung voice take (`natural`), or 0
/// when it isn't known (it then runs on its own).
fn card_need(state: &AppState, req: &Request, model: Option<&MediaModel>) -> u64 {
    match req.op.as_str() {
        "map" | "stems" | "dry" | "revoice" | "dub" => state.catalog.voices.cards.as_ref().map_or(0, |r| r.min_free_mib << 20),
        // Ditto took about 2 GB on an RTX 5070; room to spare.
        "talk" => 3 * GIB,
        // The music model makes its sound.
        "video_sound" => choose(state, "music", None, false).map_or(0, |m| (m.min_vram_gb * GIB as f64) as u64),
        _ => model.map_or(0, |m| (m.min_vram_gb * GIB as f64) as u64),
    }
}

/// Jobs that can run on the processor when started now that way (slower):
/// pictures, a song's parts, a sung voice, reading aloud. Not video or
/// music, which would take hours there.
fn on_the_processor_too(op: &str) -> bool {
    matches!(op, "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background" | "cover_art" | "map" | "stems" | "dry" | "revoice" | "narrate" | "video_edit" | "dub" | "captions")
}

/// A pose picture with only its pose left: the people as plain gray shapes
/// on white (with the background remover installed), else blurred and gray,
/// so the shape of the body shows but not who it is.
async fn pose_only(ctx: &RunCtx<'_>, img: image::DynamicImage) -> image::DynamicImage {
    let state = ctx.state;
    let remover = choose(state, "remove_background", None, false).ok();
    let dll = crate::natural::runtime_dll(&state.paths, &state.catalog.voices);
    if let (Some(m), Some(dll)) = (remover, dll) {
        if let Some(f) = m.file("onnx") {
            let (model, threads, src) = (file_path(&state.paths, &m, f), state.limits().threads.max(2), img.clone());
            let shape = tokio::task::spawn_blocking(move || cutout::matte(&dll, &model, &src, threads)).await.ok().and_then(Result::ok);
            if let Some(matte) = shape {
                let mut out = image::RgbImage::from_pixel(img.width(), img.height(), image::Rgb([255, 255, 255]));
                for (x, y, a) in matte.enumerate_pixels() {
                    if a.0[0] > 128 {
                        out.put_pixel(x, y, image::Rgb([120, 120, 120]));
                    }
                }
                return image::DynamicImage::ImageRgb8(out);
            }
        }
    }
    let side = img.width().max(img.height()) as f32;
    image::DynamicImage::ImageRgba8(image::imageops::blur(&img.grayscale().to_rgba8(), side / 60.0))
}

/// A negative prompt without the words that rule out a look (drawings,
/// cartoons, renders), keeping those against flaws.
fn without_style_words(negative: &str) -> String {
    const LOOKS: [&str; 10] = ["cartoon", "anime", "illustration", "painting", "drawing", "3d render", "cgi", "sketch", "pixel", "stylized"];
    negative.split(',').map(str::trim).filter(|w| !w.is_empty() && !LOOKS.iter().any(|l| w.to_lowercase().contains(l))).collect::<Vec<_>>().join(", ")
}

async fn run_kind(state: &Arc<AppState>, id: &str, req: &Request, model: Option<&MediaModel>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let reserved = chat_model_vram(state).await;
    let ctx = RunCtx { state, req, cancel, progress, reserved };
    let work = work_dir(&state.paths, id)?;
    match req.op.as_str() {
        "generate" | "edit" | "fill" | "extend" | "restyle" => {
            picture(&ctx, model.ok_or("No picture model.")?, &work.0).await
        }
        "upscale" => {
            upscale(&ctx, model.ok_or("No upscaler.")?, &work.0).await
        }
        "remove_background" => remove_background(&ctx, model.ok_or("No background remover.")?).await,
        "video" => {
            video(&ctx, model.ok_or("No video model.")?, &work.0).await
        }
        "speak" => Box::pin(speak(&ctx, model.ok_or("No video model.")?, &work.0)).await,
        #[cfg(windows)]
        "talk" => Box::pin(talk(&ctx, &work.0)).await,
        "revoice" => revoice_job(&ctx).await,
        "cover_art" => cover_art(&ctx, model.ok_or("No picture model.")?, &work.0).await,
        "music" | "sound" => {
            let mut items = song(&ctx, model.ok_or("No music model.")?, &work.0).await?;
            if req.music.as_ref().is_some_and(|m| m.best_of) && items.len() > 1 {
                items = keep_clearest(&ctx, items).await?;
            }
            // To be sung in a recorded voice: the first version sung again in
            // it, in its place (each version took as long again; the others
            // can be sung from the gallery).
            if let Some(voice) = req.music.as_ref().and_then(|m| m.own_voice.clone()).filter(|_| req.op == "music") {
                let made = items.clone();
                let c = state.work_cipher()?;
                for mut item in made.into_iter().filter(|i| i.lyrics.as_deref().is_some_and(lyrics::has_singing)).take(1) {
                    let at = items.iter().position(|i| i.id == item.id).unwrap_or(0);
                    match revoice_item(&ctx, &item, &voice).await {
                        // Only the song in their voice is shown: the music
                        // model's own is kept out of the gallery, reached
                        // from it as "← The original".
                        Ok(v) => {
                            item.hidden = true;
                            store::update(&state.db.lock().unwrap(), &c, &item)?;
                            items[at] = v;
                        }
                        Err(e) if e == download::CANCELLED => return Err(e),
                        Err(e) => {
                            crate::applog::line("media", &format!("The song wasn't sung again in the recorded voice: {e}"));
                            item.note = Some(format!("This wasn't sung again in your recorded voice: {e}"));
                            store::update(&state.db.lock().unwrap(), &c, &item)?;
                            items[at] = item;
                        }
                    }
                }
            }
            // Made from the song view: the new version's sections and notes,
            // read from it, so it opens with everything in place.
            if req.then_map == Some(true) {
                if let Some(first) = items.first().cloned() {
                    match map_item(&ctx, first).await {
                        Ok(mapped) => items[0] = mapped,
                        Err(e) => crate::applog::line("media", &format!("The new version wasn't mapped ({e}); See how it's built maps it")),
                    }
                }
            }
            // Cover art, made next as a job of its own: the picture model
            // gets the card once the music engine has let it go.
            if req.op == "music" && req.music.as_ref().is_some_and(|m| m.cover_art) {
                for item in &items {
                    queue_cover_art(state, item);
                }
            }
            Ok(items)
        }
        "narrate" => narrate(&ctx).await,
        "podcast" | "dialogue" => podcast::read(&ctx, &work.0).await,
        "explainer" => explainer::make(&ctx, &work.0).await,
        "listen" => listen_job(&ctx).await,
        "map" => map_job(&ctx).await,
        "stems" => stems_job(&ctx).await,
        "dry" => dry_job(&ctx).await,
        "studio_part" => studio_part(&ctx, model.ok_or("Making a part needs ACE-Step 1.5 Base (Models).")?, &work.0).await,
        #[cfg(windows)]
        "train" => train_job(&ctx, &work.0).await,
        "train_picture" => picture_train::picture_train_job(&ctx, &work.0).await,
        "video_edit" => video_edit(&ctx, &work.0).await,
        "video_sound" => video_sound(&ctx, &work.0).await,
        "dub" => Box::pin(dub(&ctx, &work.0)).await,
        "captions" => Box::pin(captions(&ctx, &work.0)).await,
        // The trainer and its files only, ahead of training.
        #[cfg(windows)]
        "trainer" => {
            let t = state.catalog.media.trainer.clone().ok_or("Training isn't available on this system yet.")?;
            ensure_trainer(&ctx, &t).await?;
            emit(state, "media:trained", json!({}));
            Ok(Vec::new())
        }
        #[cfg(windows)]
        "song_video" => song_video_job(&ctx, &work.0).await,
        "remake" => remake_job(&ctx, model.ok_or("No music model.")?, &work.0).await,
        "tune" => {
            let m = model.ok_or("No picture model.")?;
            let n = std::sync::atomic::AtomicUsize::new(0);
            let base = work.0.clone();
            // Each practice picture's own progress stays out of the bar: the
            // tuning says how far it is overall (it bounced between the two).
            let quiet = |_: &str, _: f64| {};
            let ctx = RunCtx { state, req, cancel, progress: &quiet, reserved };
            let render = |prompt: String, negative: String, seed: u64| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<image::DynamicImage, String>> + Send + '_>> {
                let dir = base.join(format!("t{}", n.fetch_add(1, Ordering::Relaxed)));
                let ctx = &ctx;
                Box::pin(async move { ctx.render(m, &prompt, &negative, seed as i64, &dir).await })
            };
            let report = prompts::tune(state, m, 3, &render, progress, cancel).await?;
            emit(state, "media:tuned", json!(report));
            Ok(Vec::new())
        }
        "coach" => {
            let m = model.ok_or("No picture model.")?;
            let n = std::sync::atomic::AtomicUsize::new(0);
            let base = work.0.clone();
            // As for tuning: the coach's own progress, not each picture's.
            let quiet = |_: &str, _: f64| {};
            let ctx = RunCtx { state, req, cancel, progress: &quiet, reserved };
            let render = |prompt: String, negative: String, seed: u64| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<image::DynamicImage, String>> + Send + '_>> {
                let dir = base.join(format!("p{}", n.fetch_add(1, Ordering::Relaxed)));
                let ctx = &ctx;
                Box::pin(async move { ctx.render(m, &prompt, &negative, seed as i64, &dir).await })
            };
            let edit = |before: image::DynamicImage, steps: Vec<String>, seed: i64| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<image::DynamicImage, String>> + Send + '_>> {
                let dir = base.join(format!("e{}", n.fetch_add(1, Ordering::Relaxed)));
                let ctx = &ctx;
                Box::pin(async move { edit_steps(ctx, m, &before, None, &steps, seed, &dir, (0.0, 1.0)).await })
            };
            let report = coach::train(state, m, &render, Some(&edit), progress, cancel).await?;
            emit(state, "media:coached", json!(report));
            Ok(Vec::new())
        }
        _ => Err("Unknown kind of job.".into()),
    }
}

/// What FFprobe finds in a file, for tests.
#[cfg(test)]
pub(crate) async fn ffmpeg_probe_for_test(ffprobe: &std::path::Path, file: &std::path::Path) -> ffmpeg::Probe {
    ffmpeg::probe(ffprobe, file).await.unwrap()
}

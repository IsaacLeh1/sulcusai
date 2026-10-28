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
pub mod prompts;
pub mod screen;
mod sd;
pub mod store;
pub mod wallpaper;
mod longclip;
#[cfg(windows)]
mod mp4;
pub(crate) mod songvideo;

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

fn sd_dir(c: &MediaCatalog, backend: &str) -> String {
    format!("sd-{}-{backend}", c.engine.build)
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
    engine::find_exe(&state.paths.engines.join(sd_dir(&state.catalog.media, backend)), SD_EXE)
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
            let asset = c.engine.assets.get(backend).ok_or("No picture engine for this PC")?;
            let dir = sd_dir(c, backend);
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
            _ => engine::backend_for(&b).is_ok_and(|be| engine::find_exe(&state.paths.engines.join(sd_dir(c, be)), SD_EXE).is_some()),
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

/// A song as the person shaped it in the Studio or asked in a chat: its
/// style, instruments, voice and beat (folded into the music model's
/// description), its tempo, key and time signature (the music model's own
/// settings), and work on a song they have.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MusicSpec {
    /// "Pop", "Jazz", "Lo-fi hip hop"…
    pub genre: Option<String>,
    /// "Happy", "Dark", "Dreamy"…
    pub mood: Option<String>,
    pub instruments: Vec<String>,
    /// The singing: "female", "male", "duet", "choir", "rap" (None: as described).
    pub voice: Option<String>,
    /// The beat's feel: "four on the floor", "trap hi-hats", "swing"…
    pub beat: Option<String>,
    /// Beats a minute (None: the music model picks).
    pub bpm: Option<u32>,
    /// "C major", "F# minor".
    pub key: Option<String>,
    /// "4/4", "3/4", "6/8".
    pub time_signature: Option<String>,
    /// Work on a song they have (`audio`): "cover" (the same song in another
    /// style), "repaint" (a part made again), "extend" (more after its end),
    /// "add" (a new part on top), "extract" (one part alone), "complete"
    /// (a full arrangement around a lone part). None or "new": a new song.
    pub task: Option<String>,
    /// The gallery's song to work on.
    pub audio: Option<String>,
    /// A song whose sound (voices and instruments) the new one follows.
    pub reference: Option<String>,
    /// The part to make again, in seconds from the start (repaint); for
    /// extending, how many seconds to add is `extend_by`.
    pub region: Option<[f32; 2]>,
    pub extend_by: Option<f32>,
    /// The part to add, take out or complete with ("vocals", "drums"…).
    pub track: Option<String>,
    /// A cover: how close to the original it stays (0 to 1).
    pub strength: Option<f32>,
    /// How the lead sings: "smooth", "powerful", "breathy", "raspy", "soulful",
    /// "falsetto", "whispered", "belting".
    pub vocal_style: Option<String>,
    /// Who sings harmonies: "female", "male", "mixed", "choir" (None: no sung harmonies).
    pub harmony_voice: Option<String>,
    /// The instrument playing a harmony line ("strings", "piano"…; None: none).
    pub harmony_instrument: Option<String>,
    /// Where harmonies come in: "chorus" (the default), "throughout", "ending".
    pub harmony_where: Option<String>,
    /// How they're sung: "thirds" (close), "octave", "stacked", "call" (call and
    /// response), "oohs".
    pub harmony_style: Option<String>,
    /// Layered on in a second pass by a model that adds parts (fuller; slower).
    pub harmony_layer: bool,
    /// The instrument that carries the melody's hook and a solo.
    pub lead_instrument: Option<String>,
    /// The song's shape: "verse-chorus", "verse-chorus-bridge", "intro-outro",
    /// "build-drop", "aaba", "story".
    pub structure: Option<String>,
    /// Sections in the order chosen ("Intro", "Verse", "Chorus", "Chorus"…);
    /// when given, they're the shape (`structure` is only a starting point).
    pub sections: Vec<String>,
    /// How lines rhyme: "aabb", "abab", "abcb", "aaaa", "free".
    pub rhyme: Option<String>,
    /// How it moves: "steady", "builds", "big-chorus", "quiet".
    pub energy: Option<String>,
    /// How it ends: "fade", "big-finish", "hard-stop".
    pub ending: Option<String>,
    /// The singer's voice in the music model's words ("warm alto with a
    /// slight rasp"): a saved artist's, the same in each of their songs.
    pub voice_detail: Option<String>,
    /// A saved artist (`persona::Persona`, by id or name): their sound and
    /// voice for this song, and the song kept as their voice if it's their first.
    pub persona: Option<String>,
    /// The length follows the song (`auto_seconds`): a length named in the
    /// description, else what the lyrics or the sections need, else what
    /// the lyrics written for it come to.
    pub auto_length: bool,
    /// Sung in a voice the person recorded (`own_voice`, by id): the song is
    /// made with their singing as its guide, then its voice is sung again in
    /// theirs (the version before kept out of the gallery, as its original).
    pub own_voice: Option<String>,
    /// Of the versions made, only the one whose words were heard best is
    /// kept (`keep_clearest`).
    pub best_of: bool,
    /// Each song gets cover art from the picture model, made next (`cover_art`).
    pub cover_art: bool,
    /// A custom model trained on the person's songs (`train`, by id): the
    /// song is made on ACE-Step 1.5 Base with it.
    pub custom: Option<String>,
    /// How strongly the custom model is followed (1 as trained).
    pub custom_scale: Option<f32>,
    /// How closely the description is followed, 0 to 1 (Suno's Style
    /// Influence): the music model's guidance (`music::guidance`). None: its own.
    pub influence: Option<f32>,
    /// How far from the usual, 0 to 1 (Suno's Weirdness): the words, the
    /// description and YuE2's melody writing. None: in between.
    pub weirdness: Option<f32>,
    /// Styles, instruments or sounds to leave out, comma separated.
    pub exclude: Option<String>,
    /// The song's name, given by the person (else its hook names it).
    pub title: Option<String>,
    /// The sound of the recording: "studio", "live", "lo-fi", "acoustic",
    /// "modern" (older songs may say a decade, as `era` does now).
    pub production: Option<String>,
    /// The decade the song is from, in its writing, instruments and sound:
    /// "1950s" to "2020s".
    pub era: Option<String>,
    /// The feeling in the words and the voice: "heartbroken", "joyful"…
    pub emotion: Option<String>,
    /// How the voice is used, and where: screamed choruses, heavy vibrato throughout.
    pub vocal_effects: Vec<VocalEffect>,
    /// An artist whose style the song follows. The name goes to the lyrics
    /// writer only; the music model gets `artist_sound`.
    pub sound_like: Option<String>,
    /// Their sound in plain words, no names (`artist::look_up` fills it).
    pub artist_sound: Option<String>,
    /// How they write lyrics, for the lyrics writer.
    pub artist_writing: Option<String>,
}

/// A way of singing, and the part of the song it's on.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct VocalEffect {
    /// "vibrato", "belting", "falsetto", "screamed", "growled", "raspy",
    /// "breathy", "whispered", "spoken", "rapped", "autotune", "runs",
    /// "operatic", or the person's own words.
    pub how: String,
    /// "all" (the default), "verse", "pre-chorus", "chorus", "bridge", "final-chorus".
    pub on: String,
}

impl VocalEffect {
    /// As the music model's description puts it ("screamed vocals").
    fn described(&self) -> String {
        let how = self.how.trim().to_lowercase();
        match how.as_str() {
            "vibrato" => "vocals with heavy vibrato".into(),
            "belting" => "powerful belted vocals".into(),
            "falsetto" => "falsetto vocals".into(),
            "screamed" => "screamed vocals, screamo".into(),
            "growled" => "growled vocals".into(),
            "raspy" => "raspy, gritty vocals".into(),
            "breathy" => "breathy vocals".into(),
            "whispered" => "whispered vocals".into(),
            "spoken" => "spoken word vocals".into(),
            "rapped" => "rapped vocals".into(),
            "autotune" => "auto-tuned vocals".into(),
            "runs" => "soulful vocal runs and riffs".into(),
            "operatic" => "operatic vocals".into(),
            other if other.contains("vocal") || other.contains("voice") => other.to_string(),
            other => format!("{other} vocals"),
        }
    }

    /// As one word on a section's tag ("[Chorus - screamed]"): one only,
    /// since more were sung as words.
    fn tag_word(&self) -> String {
        let how = self.how.trim().to_lowercase();
        match how.as_str() {
            "rapped" => "rap".into(),
            "autotune" => "auto-tuned".into(),
            "runs" => "vocal runs".into(),
            "vibrato" => "heavy vibrato".into(),
            other => other.chars().take(24).collect(),
        }
    }

    /// The part of the song, as words ("on the choruses").
    fn place(&self) -> &'static str {
        match self.on.trim().to_lowercase().as_str() {
            "verse" => "on the verses",
            "pre-chorus" => "on the pre-choruses",
            "chorus" => "on the choruses",
            "bridge" => "on the bridge",
            "final-chorus" => "on the final chorus",
            _ => "throughout",
        }
    }
}

/// Sections in order for each song shape.
const STRUCTURES: &[(&str, &[&str])] = &[
    ("verse-chorus", &["Verse", "Chorus", "Verse", "Chorus"]),
    ("verse-chorus-bridge", &["Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus"]),
    ("intro-outro", &["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"]),
    ("build-drop", &["Intro", "Verse", "Pre-Chorus", "Chorus", "Verse", "Pre-Chorus", "Chorus"]),
    ("aaba", &["Verse", "Verse", "Bridge", "Verse"]),
    ("story", &["Verse", "Verse", "Verse"]),
];

/// The music model's name for a harmony instrument's part (`music::PARTS`).
pub fn part_for(instrument: &str) -> Option<&'static str> {
    let i = instrument.trim().to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| i.contains(x));
    Some(if has(&["string", "violin", "cello", "viola"]) {
        "strings"
    } else if has(&["piano", "organ", "keys", "keyboard", "rhodes"]) {
        "keyboard"
    } else if has(&["synth", "pad"]) {
        "synth"
    } else if has(&["brass", "trumpet", "horn", "trombone"]) {
        "brass"
    } else if has(&["flute", "sax", "clarinet", "woodwind", "oboe"]) {
        "woodwinds"
    } else if has(&["guitar"]) {
        "guitar"
    } else {
        return None;
    })
}

impl MusicSpec {
    fn some(o: &Option<String>) -> Option<String> {
        o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_lowercase())
    }

    /// Who sings the harmonies, in the music model's words ("male", "male and female").
    fn harmony_singers(&self) -> Option<String> {
        Self::some(&self.harmony_voice).map(|v| match v.as_str() {
            "mixed" => "male and female".to_string(),
            other => other.to_string(),
        })
    }

    /// Where the harmonies come in, as words.
    fn harmony_place(&self) -> &'static str {
        match Self::some(&self.harmony_where).as_deref() {
            Some("throughout") => "throughout the song",
            Some("ending") => "on the final chorus",
            _ => "on the choruses",
        }
    }

    /// The sung harmonies, described ("female backing harmony vocals in close thirds on the choruses").
    pub fn harmony_vocals(&self) -> Option<String> {
        let who = self.harmony_singers()?;
        let how = match Self::some(&self.harmony_style).as_deref() {
            Some("octave") => "doubling the melody an octave apart",
            Some("stacked") => "in rich stacked gospel-style harmonies",
            Some("call") => "answering the lead in call and response",
            Some("oohs") => "singing soft oohs and aahs",
            _ => "in close harmony a third above the lead",
        };
        Some(format!("{who} backing harmony vocals {how} {}", self.harmony_place()))
    }

    /// The instrument harmony, described ("strings playing a harmony line under the melody on the choruses").
    pub fn harmony_line(&self) -> Option<String> {
        Self::some(&self.harmony_instrument).map(|i| format!("{i} playing a harmony line under the melody {}", self.harmony_place()))
    }

    /// The sections, in order, for the lyrics writer: the ones chosen, else
    /// the shape's (empty: the writer's own choice).
    pub fn sections(&self) -> Vec<String> {
        let chosen: Vec<String> = self.sections.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(24).collect();
        if !chosen.is_empty() {
            return chosen;
        }
        let s = Self::some(&self.structure).unwrap_or_default();
        STRUCTURES.iter().find(|(id, _)| *id == s).map_or(Vec::new(), |(_, list)| list.iter().map(|x| x.to_string()).collect())
    }

    /// What the lyrics writer is told beyond the song's description and its
    /// sections (`lyrics::plan`): the artist whose way of writing it follows,
    /// the feeling, how lines rhyme, lines fit for screaming or rapping, and
    /// echoes in brackets for the harmony voices.
    pub fn lyrics_guide(&self) -> String {
        let mut guide = String::new();
        if let Some(t) = self.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            guide.push_str(&format!(" The song is called \"{t}\": build the chorus's hook around those words."));
        }
        let left_out = self.excluded();
        if !left_out.is_empty() {
            guide.push_str(&format!(" Leave these out of the words: {}.", left_out.join(", ")));
        }
        match self.weirdness {
            Some(w) if w >= 0.7 => guide.push_str(" Be daring: surprising images, unusual turns of phrase and an unexpected structure."),
            Some(w) if w <= 0.25 => guide.push_str(" Keep it familiar: plain, classic images and a standard song structure."),
            _ => {}
        }
        if let Some(name) = self.sound_like.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let how = Self::some(&self.artist_writing).map(|w| format!(" ({})", w.trim_end_matches('.'))).unwrap_or_default();
            guide.push_str(&format!(" Write the way {name} writes songs{how}, in words of your own: never use lines, hooks or titles from their songs."));
        }
        if let Some(e) = Self::some(&self.emotion) {
            guide.push_str(&format!(" The words carry this feeling: {e}."));
        }
        if let Some(e) = Self::some(&self.era) {
            guide.push_str(&format!(" It's a song from the {e}: write the words the way songs were written then, with nothing in them from after that time."));
        }
        for fx in &self.vocal_effects {
            let rough = match fx.how.trim().to_lowercase().as_str() {
                "screamed" | "growled" => "screamed: make them short, raw and forceful",
                "rapped" => "rapped: give them rhythm and rhymes inside the lines, and they can be longer",
                "spoken" => "spoken, not sung",
                "whispered" => "whispered: make them soft and close",
                _ => continue,
            };
            let place = match fx.on.trim().to_lowercase().as_str() {
                "verse" => "under [Verse] tags",
                "pre-chorus" => "under [Pre-Chorus] tags",
                "chorus" => "under [Chorus] tags",
                "bridge" => "under the [Bridge] tag",
                "final-chorus" => "of the last [Chorus]",
                _ => "of the song",
            };
            guide.push_str(&format!(" The lines {place} are {rough}."));
        }
        if let Some(r) = Self::some(&self.rhyme) {
            guide.push_str(match r.as_str() {
                "aabb" => " Rhyme in pairs (AABB): lines 1 and 2 rhyme, then lines 3 and 4.",
                "abab" => " Rhyme alternate lines (ABAB): lines 1 and 3 rhyme, and lines 2 and 4.",
                "abcb" => " Rhyme the second and fourth lines of each four (ABCB).",
                "aaaa" => " Make all the lines of a section rhyme with each other (AAAA).",
                "free" => " Don't rhyme: write free verse.",
                _ => "",
            });
        }
        if self.harmony_singers().is_some() {
            let place = match Self::some(&self.harmony_where).as_deref() {
                Some("throughout") => "on some lines in every section",
                Some("ending") => "on the lines of the last chorus",
                _ => "on the chorus lines",
            };
            guide.push_str(&format!(
                " Add short backing-vocal echoes in round brackets at the end of lines {place}, for example: We'll find our way (find our way). \
                 Only the harmony singers sing what's in brackets, so keep them to two or three words."
            ));
        }
        guide
    }

    /// Lyrics with each section's way of singing on its tag ("[Chorus]"
    /// becomes "[Chorus - screamed]"): one word a tag, the part's own effect
    /// before the whole song's (those are in the description). Tags that
    /// already say something are left as they are.
    pub fn mark_sections(&self, lyrics: &str) -> String {
        let specific: Vec<&VocalEffect> = self.vocal_effects.iter().filter(|f| !f.how.trim().is_empty() && f.place() != "throughout").collect();
        if specific.is_empty() {
            return lyrics.to_string();
        }
        let last_chorus = lyrics.lines().enumerate().filter(|(_, l)| l.trim().eq_ignore_ascii_case("[chorus]")).map(|(i, _)| i).last();
        lyrics
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let t = line.trim();
                let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')).filter(|n| !n.contains('-') || n.eq_ignore_ascii_case("pre-chorus")) else {
                    return line.to_string();
                };
                // "Verse 2" is a verse.
                let kind = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ').to_lowercase();
                let fits = |f: &&&VocalEffect| match f.on.trim().to_lowercase().as_str() {
                    "final-chorus" => Some(i) == last_chorus,
                    on => on == kind,
                };
                let fx = specific.iter().find(|f| f.on.trim().eq_ignore_ascii_case("final-chorus") && fits(f)).or_else(|| specific.iter().find(|f| fits(f)));
                match fx {
                    Some(f) => format!("[{name} - {}]", f.tag_word()),
                    None => line.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Rapped rather than sung: a rap voice, a rap style (hip hop, trap,
    /// drill, grime), rapping over the whole song, or a rap asked for in
    /// the person's words.
    pub fn flow(&self, words: &str) -> lyrics::Flow {
        let voice = Self::some(&self.voice);
        let genre = Self::some(&self.genre).unwrap_or_default();
        // Lo-fi hip hop is soft singing or none.
        let style = ["hip hop", "hip-hop", "hiphop", "rap", "trap", "drill", "grime", "boom bap"].iter().any(|w| genre.contains(w)) && !["lo-fi", "lofi", "chill"].iter().any(|w| genre.contains(w));
        let rapped = self.vocal_effects.iter().any(|f| f.how.trim().eq_ignore_ascii_case("rapped") && matches!(f.on.trim(), "" | "all"));
        let w = words.to_lowercase();
        let said = w.split(|c: char| !c.is_alphanumeric() && c != '-').any(|x| matches!(x, "rap" | "raps" | "rapper" | "rapping" | "rapped" | "freestyle" | "hip-hop" | "hiphop"))
            || w.contains("hip hop");
        let rap = voice.as_deref() == Some("rap") || rapped || ((style || said) && !matches!(voice.as_deref(), Some("choir")));
        if rap {
            lyrics::Flow::Rap
        } else {
            lyrics::Flow::Sung
        }
    }

    /// Says enough about the song to make one without other words.
    pub fn describes(&self) -> bool {
        self.genre.as_deref().is_some_and(|g| !g.trim().is_empty()) || !self.instruments.is_empty()
    }

    /// The music model's description: the person's words, then the style,
    /// mood, instruments, voice, harmonies, beat and arrangement as ACE-Step's
    /// captions put them.
    pub fn caption(&self, words: &str, instrumental: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        // Said again last (the singer's sex).
        let mut tail: Option<String> = None;
        let words = words.trim().trim_end_matches('.');
        if !words.is_empty() {
            parts.push(words.to_string());
        }
        let some = |o: &Option<String>| o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(g) = some(&self.genre) {
            parts.push(g.to_lowercase());
        }
        if let Some(e) = Self::some(&self.era) {
            parts.push(format!("{e} style, a song from the {e} with the instruments and recording sound of that era"));
        }
        // An artist's sound in plain words (the music model doesn't know names).
        if let Some(a) = some(&self.artist_sound) {
            parts.push(a.trim_end_matches('.').to_string());
        }
        if let Some(m) = some(&self.mood) {
            parts.push(format!("{} mood", m.to_lowercase()));
        }
        if let Some(e) = Self::some(&self.emotion) {
            parts.push(if instrumental { format!("{e} feeling") } else { format!("{e}, emotional vocal delivery") });
        }
        let left_out = self.excluded();
        let inst: Vec<String> = self.instruments.iter().map(|i| i.trim().to_lowercase()).filter(|i| !i.is_empty() && !names_any(i, &left_out)).collect();
        if !inst.is_empty() {
            parts.push(inst.join(", "));
        }
        if let Some(l) = Self::some(&self.lead_instrument) {
            parts.push(format!("{l} lead melody and solo"));
        }
        if instrumental {
            parts.push("instrumental, no vocals".into());
        } else {
            let style = Self::some(&self.vocal_style).map(|s| format!("{s} ")).unwrap_or_default();
            let rap = self.flow(words) == lyrics::Flow::Rap;
            let singer = match some(&self.voice).map(|v| v.to_lowercase()) {
                Some(v) => Some(match v.as_str() {
                    "female" | "male" if rap => format!("{style}{v} rapper, rap vocals"),
                    "female" | "male" => format!("{style}{v} lead vocals"),
                    "rap" => format!("{style}rap vocals, rapper"),
                    "choir" => format!("{style}choir vocals"),
                    "duet" => format!("{style}male and female duet vocals"),
                    other => format!("{style}{other} vocals"),
                }),
                None if rap => Some(format!("{style}rap vocals, rapper")),
                None if !style.is_empty() => Some(format!("{style}vocals")),
                None => None,
            };
            // Who sings goes first after the person's words, and a man or a
            // woman is said again at the end: placed among the instruments it
            // was lost (a male voice asked for came out female). ACE-Step's
            // guide: repeat what matters most.
            if let Some(s) = singer {
                parts.insert(usize::from(!words.is_empty()), s);
                if let Some(v) = some(&self.voice).map(|v| v.to_lowercase()).filter(|v| v == "male" || v == "female") {
                    let who = if v == "male" { "man" } else { "woman" };
                    tail = Some(format!("sung by a {who}, {v} voice throughout"));
                }
            }
            if let Some(d) = some(&self.voice_detail) {
                parts.push(d.trim_end_matches('.').to_string());
            }
            // Rapped, not sung: said plainly, with the flow (rap sung as a
            // melody came out half-sung and thin).
            if rap {
                parts.push("rapping with a tight rhythmic flow, spoken rhymes on the beat, not sung".into());
            }
            for fx in self.vocal_effects.iter().filter(|f| !f.how.trim().is_empty()) {
                parts.push(format!("{} {}", fx.described(), fx.place()));
            }
            if let Some(h) = self.harmony_vocals() {
                parts.push(h);
            }
        }
        if let Some(h) = self.harmony_line() {
            parts.push(h);
        }
        if let Some(b) = some(&self.beat) {
            parts.push(format!("{} beat", b.to_lowercase()));
        }
        if instrumental && !self.sections().is_empty() {
            let named: Vec<String> = self.sections().iter().map(|s| s.to_lowercase()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            parts.push(format!("arranged with {}", named.join(", ")));
        }
        if let Some(e) = Self::some(&self.energy) {
            parts.push(match e.as_str() {
                "builds" => "builds up gradually".into(),
                "big-chorus" => "soft verses and a big, powerful chorus".into(),
                "quiet" => "quiet and intimate".into(),
                "steady" => "steady energy".into(),
                other => other.to_string(),
            });
        }
        if let Some(p) = Self::some(&self.production) {
            parts.push(match p.as_str() {
                "studio" => "polished studio production".into(),
                "live" => "live band recording".into(),
                "lo-fi" => "lo-fi production".into(),
                "acoustic" => "acoustic, unplugged".into(),
                "modern" => "modern production".into(),
                decade if decade.ends_with('s') => format!("{decade} production"),
                other => format!("{other} production"),
            });
        }
        if let Some(e) = Self::some(&self.ending) {
            parts.push(match e.as_str() {
                "fade" => "fades out at the end".into(),
                "big-finish" => "ends with a big finish".into(),
                "hard-stop" => "ends abruptly".into(),
                other => other.to_string(),
            });
        }
        // Weirdness, Suno's way: far from the usual at the top, by the book
        // at the bottom.
        match self.weirdness {
            Some(w) if w >= 0.7 => parts.push("experimental, unexpected arrangement, unusual sounds and surprising turns".into()),
            Some(w) if w <= 0.25 => parts.push("classic, familiar arrangement".into()),
            _ => {}
        }
        // What's left out: no part that names it (the person's own words
        // stay as written).
        if !left_out.is_empty() {
            let own = usize::from(!words.is_empty());
            let mut i = 0;
            parts.retain(|p| {
                i += 1;
                i <= own || !names_any(p, &left_out)
            });
        }
        parts.extend(tail);
        parts.join(", ")
    }

    /// What's to be left out, each lowercased without a leading "no".
    pub fn excluded(&self) -> Vec<String> {
        let list = self.exclude.as_deref().unwrap_or_default();
        list.split([',', ';', '\n'])
            .map(|s| {
                let s = s.trim().to_lowercase();
                s.strip_prefix("no ").or_else(|| s.strip_prefix("without ")).unwrap_or(&s).trim().to_string()
            })
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Whether a phrase names any of `terms` as whole words ("drums" is in
/// "punchy live drums", not in "drumsticks").
fn names_any(phrase: &str, terms: &[String]) -> bool {
    let norm = |s: &str| format!(" {} ", s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "));
    let p = norm(phrase);
    terms.iter().any(|t| {
        let t = norm(t);
        t.trim().len() > 1 && p.contains(&t)
    })
}

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

// ---------- video edits ----------

/// What to do to a video (`video_edit`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct VideoEdit {
    /// Keep only this stretch of it (seconds from its start).
    pub trim: Option<[f64; 2]>,
    /// Other clips (gallery ids) joined after it, in order.
    pub join: Vec<String>,
    /// A sound track: a song or sound from the gallery, or a file.
    pub audio: Option<String>,
    pub audio_file: Option<String>,
    /// "replace" its sound (the default) or "mix" with it.
    pub audio_mode: Option<String>,
    /// The sound track's loudness (1 as it is).
    pub volume: Option<f32>,
    /// Its sound taken off.
    pub mute: bool,
}

/// A gallery item written out for FFmpeg.
fn written_out(state: &AppState, c: &crate::crypto::Cipher, item: &MediaItem, to: &std::path::Path) -> Result<PathBuf, String> {
    let ext = match item.mime.as_str() {
        m if m.contains("webm") => "webm",
        m if m.contains("mp4") => "mp4",
        m if m.contains("wav") => "wav",
        m if m.contains("mpeg") => "mp3",
        m if m.contains("ogg") => "ogg",
        _ => if item.kind == "video" { "mp4" } else { "wav" },
    };
    let path = to.with_extension(ext);
    std::fs::write(&path, store::read(&state.paths, c, &item.id)?).map_err(|e| e.to_string())?;
    Ok(path)
}

async fn video_edit(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let edit = ctx.req.edit.clone().unwrap_or_default();
    Ok(vec![edit_video(ctx, work, &id, &edit, "video_edit").await?])
}

/// A video with `edit` done to it, kept as a new video beside it.
async fn edit_video(ctx: &RunCtx<'_>, work: &std::path::Path, id: &str, edit: &VideoEdit, op: &str) -> Result<MediaItem, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That video is no longer in the gallery.")?;
    if item.kind != "video" {
        return Err("Pick a video.".into());
    }
    let (ffmpeg, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Getting the clips ready", 0.05);
    let mut clips = vec![written_out(state, &c, &item, &work.join("clip0"))?];
    for (i, other) in edit.join.iter().enumerate() {
        let o = store::get(&state.db.lock().unwrap(), &c, other).ok_or("A clip to join is no longer in the gallery.")?;
        if o.kind != "video" {
            return Err("Only videos can be joined.".into());
        }
        clips.push(written_out(state, &c, &o, &work.join(format!("clip{}", i + 1)))?);
    }
    let mut probes = Vec::new();
    for p in &clips {
        probes.push(ffmpeg::probe(&ffprobe, p).await?);
    }
    // The first clip cut to its stretch.
    let trim = edit.trim.filter(|[a, b]| b > a).map(|[a, b]| [a.max(0.0), b.min(probes[0].seconds.max(b))]);
    if let Some([a, b]) = trim {
        probes[0].seconds = (b - a).max(0.1);
    }
    let total: f64 = probes.iter().map(|p| p.seconds).sum();
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    for (i, p) in clips.iter().enumerate() {
        if let (0, Some([a, b])) = (i, trim) {
            args.extend([format!("-ss"), format!("{a:.3}"), format!("-to"), format!("{b:.3}")].map(Into::into));
        }
        args.extend(["-i".into(), p.as_os_str().to_owned()]);
    }
    // A sound track, after the clips.
    let track = match (&edit.audio, &edit.audio_file) {
        (Some(aid), _) => {
            let a = store::get(&state.db.lock().unwrap(), &c, aid).ok_or("That sound is no longer in the gallery.")?;
            Some(written_out(state, &c, &a, &work.join("track"))?)
        }
        (None, Some(f)) => Some(PathBuf::from(f)),
        _ => None,
    };
    let mut filter = ffmpeg::join_filter(&probes);
    let audio_out = match &track {
        Some(t) => {
            args.extend(["-i".into(), t.as_os_str().to_owned()]);
            let k = clips.len();
            let vol = edit.volume.unwrap_or(1.0).clamp(0.0, 2.0);
            if edit.audio_mode.as_deref() == Some("mix") && !edit.mute {
                filter.push_str(&format!(";[{k}:a]aresample=44100,aformat=channel_layouts=stereo,volume={vol:.2}[t];[a][t]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[aout]"));
            } else {
                // The clips' own sound goes unused (FFmpeg refuses an output left unconnected).
                filter.push_str(&format!(";[a]anullsink;[{k}:a]aresample=44100,aformat=channel_layouts=stereo,volume={vol:.2},apad,atrim=duration={total:.3}[aout]"));
            }
            Some("[aout]")
        }
        None if edit.mute => {
            filter.push_str(";[a]anullsink");
            None
        }
        None => Some("[a]"),
    };
    let out = work.join("edited.mp4");
    args.extend(["-filter_complex".into(), filter.into(), "-map".into(), "[v]".into()]);
    match audio_out {
        Some(a) => args.extend(["-map".into(), a.into()]),
        None => args.push("-an".into()),
    }
    args.extend(["-t".into(), format!("{total:.3}").into()]);
    args.extend(ffmpeg::mp4_out(&out));
    let stage = if op == "dub" { "Putting the new voice in" } else { "Making the video" };
    (ctx.progress)(stage, 0.1);
    ffmpeg::run(state, &ffmpeg, args, total, ctx.cancel, &|f| (ctx.progress)(stage, 0.1 + 0.85 * f)).await?;
    // A still from it for the gallery.
    let still = work.join("still.jpg");
    let _ = ffmpeg::run(state, &ffmpeg, ["-ss", &format!("{:.2}", (total / 3.0).min(2.0)), "-i"].iter().map(Into::into).chain([out.as_os_str().to_owned(), "-frames:v".into(), "1".into(), "-q:v".into(), "3".into(), still.as_os_str().to_owned()]).collect(), 0.0, ctx.cancel, &|_| {}).await;
    let thumb = std::fs::read(&still).ok().and_then(|b| imaging::decode(&b).ok()).and_then(|img| imaging::thumbnail(&img).ok());
    let bytes = std::fs::read(&out).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let made = MediaItem {
        kind: "video".into(),
        op: op.into(),
        prompt: item.prompt.clone(),
        model_id: item.model_id.clone(),
        mime: "video/mp4".into(),
        seconds: total,
        width: probes[0].width,
        height: probes[0].height,
        parent: Some(item.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| item.chat_id.clone()),
        poster: thumb.is_some(),
        ..Default::default()
    };
    store::save(&state.db.lock().unwrap(), &state.paths, &c, made, &bytes, thumb.as_deref())
}

/// A sound made for a video by the music model (a sound effect for a short
/// clip, instrumental music for a longer one), from its description or the
/// one given, as long as the video; then put on it (mixed with its sound
/// when it has some).
async fn video_sound(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let (_, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let probe = ffmpeg::probe(&ffprobe, &written_out(state, &c, &item, &work.join("look"))?).await?;
    let secs = probe.seconds.max(item.seconds).max(1.0);
    let op = if secs <= 30.0 { "sound" } else { "music" };
    let model = choose(state, op, None, false).map_err(|_| "Making a sound for a video needs a music model (Studio › Models).".to_string())?;
    let described = if ctx.req.prompt.trim().is_empty() { format!("the sound of this scene: {}", item.prompt) } else { ctx.req.prompt.trim().to_string() };
    let ask = Request { op: op.into(), prompt: described, seconds: Some(secs.ceil().max(10.0) as f32), instrumental: Some(true), chat_id: ctx.req.chat_id.clone(), ..Default::default() };
    let stage = |s: &str, f: f64| (ctx.progress)(s, 0.6 * f);
    let step = RunCtx { state, req: &ask, cancel: ctx.cancel, progress: &stage, reserved: ctx.reserved };
    let made = Box::pin(song(&step, &model, work)).await?;
    let sound = made.into_iter().find(|i| i.kind == "audio").ok_or("The music model didn't make a sound.")?;
    let edit = VideoEdit { audio: Some(sound.id.clone()), audio_mode: Some(if probe.audio { "mix".into() } else { "replace".into() }), volume: Some(if probe.audio { 0.7 } else { 1.0 }), ..Default::default() };
    let progress = |s: &str, f: f64| (ctx.progress)(s, 0.6 + 0.4 * f);
    let rest = RunCtx { state, req: ctx.req, cancel: ctx.cancel, progress: &progress, reserved: ctx.reserved };
    Ok(vec![edit_video(&rest, work, &id, &edit, "video_sound").await?])
}

/// What's said in a video and when (seconds), for dubbing: whisper's pieces
/// joined into sentences, none past 12 s, so the new voice keeps in time.
fn dub_lines(segments: Vec<crate::speech::Segment>) -> Vec<(f64, f64, String)> {
    let mut out: Vec<(f64, f64, String)> = Vec::new();
    for s in segments {
        let text = s.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if let Some(prev) = out.last_mut() {
            let ended = prev.2.ends_with(['.', '!', '?', '…', '。', '？', '！']);
            if !ended && s.start - prev.1 < 0.6 && s.end - prev.0 <= 12.0 {
                prev.2.push(' ');
                prev.2.push_str(&text);
                prev.1 = s.end;
                continue;
            }
        }
        out.push((s.start, s.end, text));
    }
    out
}

/// How much faster a line `said` seconds long goes to fit in `room`: 1 when
/// it fits, at most 1.5 (past that the words blur).
fn dub_speed(said: f64, room: f64) -> f64 {
    if room <= 0.0 || said <= room * 1.03 {
        1.0
    } else {
        (said / room).min(1.5)
    }
}

/// The dub's voice choice that gives everyone in the video their own voice.
const EACH_VOICE: &str = "each";

/// Who says each line (1, 2, …) by the sound of their voice (`voice16`, the
/// speech at 16 kHz): the speaker model, got the first time (~30 MB);
/// everyone as one without it. Lines too short to judge go with the nearest
/// line that isn't, and someone heard for under 3 s with whoever sounds most
/// like them (too little to make a voice from).
async fn dub_speakers(ctx: &RunCtx<'_>, voice16: &[f32], lines: &[(f64, f64, String)]) -> Vec<u32> {
    let state = ctx.state;
    if lines.len() < 2 {
        return vec![1; lines.len()];
    }
    if !crate::diarize::available(state) {
        if let Err(e) = crate::diarize::install(state, ctx.cancel, &|_, _, _| (ctx.progress)("Getting the speaker model (once)", 0.55)).await {
            state.log("media", &format!("Dubbing: the speaker model didn't install ({e}); everyone gets one voice"));
            return vec![1; lines.len()];
        }
    }
    let rate = crate::audio::RATE as f64;
    let prints: Vec<Option<Vec<f32>>> = tokio::task::block_in_place(|| {
        lines
            .iter()
            .map(|(s, e, _)| {
                let (a, b) = (((s * rate) as usize).min(voice16.len()), ((e * rate) as usize).min(voice16.len()));
                (e - s >= crate::diarize::MIN_SECS).then(|| crate::diarize::embed(state, &voice16[a..b]).ok()).flatten()
            })
            .collect()
    });
    let known: Vec<usize> = (0..lines.len()).filter(|i| prints[*i].is_some()).collect();
    if known.is_empty() {
        return vec![1; lines.len()];
    }
    let grouped = crate::diarize::regroup(&known.iter().filter_map(|i| prints[*i].clone()).collect::<Vec<_>>());
    let mut labels: Vec<u32> = (0..lines.len())
        .map(|i| {
            let near = (0..known.len()).min_by(|a, b| (lines[known[*a]].0 - lines[i].0).abs().total_cmp(&(lines[known[*b]].0 - lines[i].0).abs())).unwrap_or(0);
            grouped[near]
        })
        .collect();
    // Each speaker's voice on average, and how long they speak.
    let speakers = grouped.iter().copied().max().unwrap_or(1);
    let mean = |k: u32| -> Vec<f32> {
        let mut m = vec![0f32; prints[known[0]].as_ref().map_or(0, Vec::len)];
        for (i, _) in known.iter().zip(&grouped).filter(|(_, g)| **g == k) {
            for (x, y) in m.iter_mut().zip(prints[*i].iter().flatten()) {
                *x += y;
            }
        }
        let n = m.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        m.into_iter().map(|x| x / n).collect()
    };
    let heard = |k: u32, labels: &[u32]| lines.iter().zip(labels).filter(|(_, l)| **l == k).map(|(l, _)| l.1 - l.0).sum::<f64>();
    let means: Vec<Vec<f32>> = (1..=speakers).map(mean).collect();
    for k in 1..=speakers {
        if speakers > 1 && heard(k, &labels) < 3.0 {
            let like = (1..=speakers).filter(|o| *o != k && heard(*o, &labels) >= 3.0).max_by(|a, b| crate::diarize::cosine(&means[k as usize - 1], &means[*a as usize - 1]).total_cmp(&crate::diarize::cosine(&means[k as usize - 1], &means[*b as usize - 1])));
            if let Some(o) = like {
                labels.iter_mut().filter(|l| **l == k).for_each(|l| *l = o);
            }
        }
    }
    labels
}

/// The dub's lines read in each speaker's own voice, made from how they
/// sound in the video: their longest lines (~10 to 15 s together, with the
/// words heard in them) taken from the speech alone (the video's sound less
/// its music and other sounds, when the separator took those out).
async fn each_own_voice(ctx: &RunCtx<'_>, mix: &stems::Stereo, background: Option<&[Vec<f32>; 2]>, lines: &[(f64, f64, String)], words: &[String], to: &str) -> Result<(Vec<Vec<f32>>, u32), String> {
    let state = ctx.state;
    let speech: Vec<f32> = match background {
        Some([l, r]) => mix.ch[0].iter().zip(&mix.ch[1]).zip(l.iter().zip(r)).map(|((a, b), (c, d))| (a + b - c - d) * 0.5).collect(),
        None => mix.mono(),
    };
    let labels = dub_speakers(ctx, &stems::resample(&speech, stems::RATE, crate::audio::RATE), lines).await;
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let at24 = stems::resample(&speech, stems::RATE, own_voice::TTS_RATE);
    let rate = own_voice::TTS_RATE as f64;
    let speakers = labels.iter().copied().max().unwrap_or(1);
    let who: Vec<u32> = (1..=speakers).filter(|k| labels.contains(k)).collect();
    let mut out = vec![Vec::new(); lines.len()];
    for (n, k) in who.iter().enumerate() {
        let theirs: Vec<usize> = (0..lines.len()).filter(|i| labels[*i] == *k).collect();
        // Their longest lines, in the order said.
        let mut longest = theirs.clone();
        longest.sort_by(|a, b| (lines[*b].1 - lines[*b].0).total_cmp(&(lines[*a].1 - lines[*a].0)));
        let mut chosen: Vec<usize> = Vec::new();
        let mut secs = 0.0;
        for i in longest {
            let d = lines[i].1 - lines[i].0;
            if secs >= 10.0 || (!chosen.is_empty() && secs + d > 15.0) {
                continue;
            }
            chosen.push(i);
            secs += d;
        }
        chosen.sort_unstable();
        let gap = vec![0f32; (rate * 0.25) as usize];
        let mut reference = Vec::new();
        for i in &chosen {
            let (a, b) = (((lines[*i].0 * rate) as usize).min(at24.len()), ((lines[*i].1 * rate) as usize).min(at24.len()));
            if !reference.is_empty() {
                reference.extend_from_slice(&gap);
            }
            reference.extend_from_slice(&at24[a..b]);
        }
        let level = crate::audio::rms(&reference);
        if level > 1e-4 {
            reference.iter_mut().for_each(|x| *x = (*x * 0.08 / level).clamp(-1.0, 1.0));
        }
        let said: String = chosen.iter().map(|i| lines[*i].2.as_str()).collect::<Vec<_>>().join(" ");
        let passages: Vec<String> = theirs.iter().map(|i| words[*i].clone()).collect();
        let (from, span) = (0.55 + 0.25 * n as f64 / who.len() as f64, 0.25 / who.len() as f64);
        let label = if who.len() > 1 { format!("Speaking it in voice {} of {}", n + 1, who.len()) } else { "Speaking it in their own voice".to_string() };
        let (read, _) = own_voice::speak_each_like(state, &reference, &said, &passages, Some(to), ctx.cancel, &|_, f| (ctx.progress)(&label, from + span * f)).await?;
        for (i, r) in theirs.iter().zip(read) {
            out[*i] = r;
        }
    }
    state.log("media", &format!("Dubbing in each speaker's own voice: {} speaker(s) heard", who.len()));
    Ok((out, own_voice::TTS_RATE))
}

/// A sound file as stereo at the separator's rate, read by FFmpeg.
async fn ffmpeg_stereo(state: &AppState, ffmpeg_exe: &std::path::Path, file: &std::path::Path, seconds: f64, cancel: &AtomicBool) -> Result<stems::Stereo, String> {
    let raw = file.with_extension("f32");
    let rate = stems::RATE.to_string();
    let args: Vec<std::ffi::OsString> = ["-i".into(), file.as_os_str().to_owned()].into_iter().chain(["-vn", "-ac", "2", "-ar", &rate, "-f", "f32le"].map(Into::into)).chain([raw.as_os_str().to_owned()]).collect();
    ffmpeg::run(state, ffmpeg_exe, args, seconds, cancel, &|_| {}).await?;
    let bytes = std::fs::read(&raw).map_err(|e| format!("FFmpeg didn't read the sound: {e}"))?;
    let _ = std::fs::remove_file(&raw);
    let all: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    Ok(stems::Stereo { rate: stems::RATE, ch: [all.iter().step_by(2).copied().collect(), all.iter().skip(1).step_by(2).copied().collect()] })
}

/// Samples at the separator's rate, `speed` times faster at the same pitch.
async fn faster(state: &AppState, ffmpeg_exe: &std::path::Path, x: &[f32], speed: f64, at: &std::path::Path, cancel: &AtomicBool) -> Result<Vec<f32>, String> {
    let (inp, out) = (at.with_extension("in.wav"), at.with_extension("out.wav"));
    std::fs::write(&inp, crate::audio::wav_encode(x, stems::RATE)).map_err(|e| e.to_string())?;
    let args: Vec<std::ffi::OsString> = ["-i".into(), inp.as_os_str().to_owned(), "-filter:a".into(), format!("atempo={speed:.3}").into(), out.as_os_str().to_owned()].into();
    ffmpeg::run(state, ffmpeg_exe, args, 0.0, cancel, &|_| {}).await?;
    Ok(ffmpeg_stereo(state, ffmpeg_exe, &out, 0.0, cancel).await?.mono())
}

/// A video's speech heard, translated and said again in another language,
/// in time with it: each line in its own stretch (a little faster when the
/// new words run longer), over the video's music and other sounds (the
/// part separator takes the old voice out) or, without the separator, over
/// its sound turned low while someone speaks. Read by the voice picked (a
/// recorded one is "own:<id>") or one that speaks the language.
async fn dub(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let to = ctx.req.language.clone().filter(|l| !l.is_empty() && l != "auto").ok_or("Pick the language to dub it into.")?;
    let into = crate::translate::language_name(&to);
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let video = written_out(state, &c, &item, &work.join("source"))?;
    let probe = ffmpeg::probe(&ffprobe, &video).await?;
    if !probe.audio {
        return Err("This video has no sound, so there's nothing to dub.".into());
    }
    (ctx.progress)("Listening to what's said", 0.05);
    let mix = ffmpeg_stereo(state, &ffmpeg_exe, &video, probe.seconds, ctx.cancel).await?;
    let speech = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.map_err(|e| format!("Dubbing needs a speech model to hear what's said. {e}"))?;
    let options = crate::speech::Options { language: Some("auto".into()), ..Default::default() };
    let heard = crate::speech::transcribe(&speech, &stems::resample(&mix.mono(), stems::RATE, crate::audio::RATE), &options).await?;
    let lines = dub_lines(crate::speech::clean(heard).segments);
    if lines.is_empty() {
        return Err("No one speaks in this video, so there's nothing to dub.".into());
    }
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }

    (ctx.progress)("Translating what's said", 0.2);
    let said: Vec<String> = lines.iter().map(|l| l.2.clone()).collect();
    let words = {
        let (ep, _, _hold) = crate::background_endpoint(state).await.map_err(|e| format!("Translating what's said needs a chat model: {e}"))?;
        crate::translate::translate_lines(&ep, &said, &to).await?
    };

    // Its music and other sounds without the voice.
    (ctx.progress)("Taking the voice out", 0.35);
    let background = separated(ctx, &mix, &[0, 1, 2], "the old sound is turned down under the new voice instead").await?;

    let speaking = format!("Speaking it in {into}");
    (ctx.progress)(&speaking, 0.55);
    let picked = ctx.req.voice.as_deref().filter(|v| *v != EACH_VOICE);
    let mut cloned = None;
    if ctx.req.voice.as_deref() == Some(EACH_VOICE) {
        if !own_voice::speaks(&to) {
            state.log("media", &format!("Dubbing: voices made from the speakers don't speak {into}; using a voice that does"));
        } else {
            match each_own_voice(ctx, &mix, background.as_ref(), &lines, &words, &to).await {
                Ok(v) => cloned = Some(v),
                Err(e) if e == download::CANCELLED => return Err(e),
                Err(e) => state.log("media", &format!("Dubbing in each speaker's own voice didn't work ({e}); using a voice that speaks {into}")),
            }
        }
    }
    let (spoken, voice_rate) = match (cloned, picked.and_then(|v| v.strip_prefix("own:"))) {
        (Some(v), _) => v,
        (None, Some(own)) => own_voice::speak_each(state, own, &words, Some(&to), ctx.cancel, &|s, f| (ctx.progress)(s, 0.55 + 0.25 * f)).await?,
        (None, None) => tokio::task::block_in_place(|| -> Result<(Vec<Vec<f32>>, u32), String> {
            let voices = crate::tts::voices()?;
            let voice = crate::tts::pick_voice(&voices, picked, Some(&to)).map(|v| v.id.clone()).ok_or("No voices are installed.")?;
            let (mut out, mut rate) = (Vec::new(), 16_000);
            for (i, w) in words.iter().enumerate() {
                if ctx.cancel.load(Ordering::Relaxed) {
                    return Err(download::CANCELLED.into());
                }
                (ctx.progress)(&speaking, 0.55 + 0.25 * i as f64 / words.len() as f64);
                let s = crate::tts::synthesize(&crate::tts::speakable(w), Some(&voice), Some(&to), 1.0)?;
                rate = s.rate;
                out.push(s.samples.iter().map(|x| *x as f32 / 32768.0).collect());
            }
            Ok((out, rate))
        })?,
    };

    // Each line at its time, as loud as speech usually is.
    (ctx.progress)("Putting the new voice in", 0.8);
    let sr = stems::RATE as f64;
    let total = mix.len();
    let ducked = background.is_none();
    let [mut l, mut r] = background.unwrap_or_else(|| mix.ch.clone());
    let mut gain = vec![1.0f32; if ducked { total } else { 0 }];
    let mut voice = vec![0.0f32; total];
    for (i, ((start, end, _), said)) in lines.iter().zip(&spoken).enumerate() {
        let next = lines.get(i + 1).map_or(probe.seconds.max(*end), |n| n.0);
        let room = (next - start).max(end - start).max(0.3);
        let mut v = stems::resample(said, voice_rate, stems::RATE);
        let speed = dub_speed(v.len() as f64 / sr, room);
        if speed > 1.0 {
            v = faster(state, &ffmpeg_exe, &v, speed, &work.join(format!("line{i}")), ctx.cancel).await?;
        }
        let rms = (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt();
        let level = if rms > 1e-4 { 0.1 / rms } else { 1.0 };
        let at = (start * sr) as usize;
        for (k, x) in v.iter().enumerate() {
            if let Some(s) = voice.get_mut(at + k) {
                *s += x * level;
            }
        }
        if ducked {
            // The old sound turned down under the line, with short fades.
            let fade = (0.15 * sr) as usize;
            for s in at.saturating_sub(fade)..(at + v.len() + fade).min(total) {
                let away = if s < at { (at - s) as f32 } else { s.saturating_sub(at + v.len()) as f32 };
                gain[s] = gain[s].min(0.2 + 0.8 * (away / fade as f32).min(1.0));
            }
        }
    }
    for s in 0..total {
        let g = gain.get(s).copied().unwrap_or(1.0);
        l[s] = l[s] * g + voice[s];
        r[s] = r[s] * g + voice[s];
    }
    let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
    if peak > 0.98 {
        let k = 0.98 / peak;
        l.iter_mut().chain(r.iter_mut()).for_each(|x| *x *= k);
    }
    let track = work.join("dubbed.wav");
    std::fs::write(&track, own_voice::wav_stereo(&l, &r, stems::RATE)).map_err(|e| e.to_string())?;
    let edit = VideoEdit { audio_file: Some(track.to_string_lossy().into_owned()), audio_mode: Some("replace".into()), volume: Some(1.0), ..Default::default() };
    let progress = |s: &str, f: f64| (ctx.progress)(s, 0.85 + 0.15 * f);
    let rest = RunCtx { state, req: ctx.req, cancel: ctx.cancel, progress: &progress, reserved: ctx.reserved };
    let made = edit_video(&rest, work, &id, &edit, "dub").await?;
    state.log("media", &format!("Dubbed a video into {into} on this PC ({} lines{})", lines.len(), if ducked { ", the old sound turned down" } else { "" }));
    Ok(vec![made])
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

/// What a job does, for the question when the card can't take it.
fn op_what(op: &str) -> &'static str {
    match op {
        "generate" | "edit" | "fill" | "extend" | "restyle" | "cover_art" => "Making the picture",
        "upscale" => "Making the picture bigger",
        "video" => "Making the video",
        "speak" | "talk" => "Making the picture talk",
        "music" | "remake" | "studio_part" => "Making the song",
        "sound" => "Making the sound",
        "map" | "stems" | "dry" => "Taking the song apart",
        "revoice" => "Singing the song in another voice",
        "tune" | "coach" => "Working on the tune",
        "train" => "Training a custom model",
        "video_edit" => "Editing the video",
        "video_sound" => "Making sound for the video",
        "dub" => "Dubbing the video",
        "captions" => "Adding captions",
        "trainer" => "Getting the trainer",
        _ => "This job",
    }
}


fn emit(state: &AppState, event: &str, payload: serde_json::Value) {
    if let Some(app) = state.app.get() {
        app.emit(event, payload).ok();
    }
}

fn update(state: &AppState, id: &str, f: impl FnOnce(&mut JobInfo)) {
    let info = {
        let mut list = jobs().lock().unwrap();
        let Some(j) = list.iter_mut().find(|j| j.info.id == id) else { return };
        f(&mut j.info);
        j.info.clone()
    };
    emit(state, "media:progress", json!(info));
}

/// A job made on the compute PC: shown here as it goes, and what it makes
/// kept here.
async fn on_compute(state: &Arc<AppState>, id: String, req: Request) -> Result<Vec<MediaItem>, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let pc = crate::sync::compute::peer_name(state).unwrap_or_else(|| "the compute PC".into());
    let info = JobInfo {
        id: id.clone(),
        op: req.op.clone(),
        prompt: req.prompt.chars().take(200).collect(),
        model_id: req.model_id.clone(),
        status: "running".into(),
        stage: format!("Sending it to {pc}"),
        fraction: 0.0,
        eta_secs: None,
        est_secs: 60.0,
        chat_id: req.chat_id.clone(),
        on: Some("compute PC".into()),
        device: Some(pc.clone()),
        gpu_usage: None,
        gpu_temp: None,
        cpu_usage: None,
        ram_usage: None,
        gpu_mem: None,
    };
    jobs().lock().unwrap().push(Job { info: info.clone(), cancel: cancel.clone() });
    emit(state, "media:progress", json!(info));
    struct Done(String);
    impl Drop for Done {
        fn drop(&mut self) {
            jobs().lock().unwrap().retain(|j| j.info.id != self.0);
        }
    }
    let _done = Done(id.clone());
    let progress = |stage: &str, fraction: f64, device: Option<String>| {
        update(state, &id, |i| {
            i.stage = stage.to_string();
            i.fraction = fraction.clamp(0.0, 1.0);
            if let Some(d) = &device {
                i.device = Some(format!("{d} on {pc}"));
            }
        });
    };
    let made = crate::sync::compute::media(state, &req, &cancel, &progress).await?;
    let c = state.work_cipher()?;
    let mut out = Vec::new();
    for (item, data, thumb) in made {
        let item = MediaItem { id: String::new(), chat_id: req.chat_id.clone(), parent: req.source.clone(), hidden: false, ..item };
        out.push(store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &data, thumb.as_deref())?);
    }
    state.log("media", &format!("Made {} on {pc}", out.len()));
    Ok(out)
}

/// Checks a request before it's queued, so mistakes show at once.
/// What a cloud model can do with a job.
fn cloud_can(c: &crate::cloud::MediaChoice, op: &str) -> bool {
    use crate::cloud::MediaKind;
    let edits = c.id.contains(":gpt-image") || c.id.contains(":dall-e-2");
    match (c.kind, op) {
        (MediaKind::Image, "generate") => true,
        (MediaKind::Image, "edit" | "restyle" | "fill") => edits,
        (MediaKind::Video, "video") => true,
        (MediaKind::Speech, "narrate") => true,
        _ => false,
    }
}

/// The cloud model a job runs on: the one asked for, or one that can do it
/// when no model on this PC can (narration always has this PC's voices).
fn cloud_pick(state: &AppState, req: &Request) -> Option<String> {
    if let Some(id) = req.model_id.as_deref().filter(|m| crate::cloud::is_cloud(m)) {
        return Some(id.to_string());
    }
    if req.op == "narrate" || choose(state, model_op(req), req.model_id.as_deref(), req.source.is_some()).is_ok() {
        return None;
    }
    crate::cloud::media_choices(&state.db.lock().unwrap()).into_iter().find(|c| cloud_can(c, &req.op)).map(|c| c.id)
}

/// The online model a job would go to, with its provider: (provider, model name).
pub(crate) fn cloud_target(state: &AppState, req: &Request) -> Option<(String, String)> {
    let id = cloud_pick(state, req)?;
    crate::cloud::media_choices(&state.db.lock().unwrap()).into_iter().find(|c| c.id == id).map(|c| (c.provider, c.name))
}

/// Any model, here or in the cloud, can make this kind of thing.
pub fn can_make(state: &AppState, kind: Kind) -> bool {
    use crate::cloud::MediaKind;
    has_kind(state, kind)
        || crate::cloud::media_choices(&state.db.lock().unwrap()).iter().any(|c| match kind {
            Kind::Image => c.kind == MediaKind::Image,
            Kind::Video => c.kind == MediaKind::Video,
            _ => false,
        })
}

/// The recorded voice (`own_voice`) a job uses: the one a song is sung again
/// in ("revoice", `voice`), narration's "own:<id>", or a new song's.
fn recorded_voice(req: &Request) -> Option<&str> {
    match req.op.as_str() {
        "revoice" => req.voice.as_deref(),
        "narrate" => req.voice.as_deref().and_then(|v| v.strip_prefix("own:")),
        "music" => req.music.as_ref().and_then(|m| m.own_voice.as_deref()),
        "podcast" | "dialogue" | "explainer" => req.talk.as_ref().and_then(|t| t.speakers.iter().find_map(|s| s.voice.as_deref().and_then(|v| v.strip_prefix("own:")))),
        _ => None,
    }
}

/// Jobs that need no media model: speech, and work on songs already made.
fn no_model(op: &str) -> bool {
    matches!(op, "narrate" | "podcast" | "dialogue" | "explainer" | "listen" | "map" | "revoice" | "stems" | "dry" | "song_video" | "train" | "trainer" | "video_edit" | "video_sound" | "dub" | "captions" | "talk")
}

pub fn check(state: &AppState, req: &Request) -> Result<(), String> {
    let f = op_feature(&req.op).ok_or("Unknown kind of job.")?;
    crate::features::require(state, f)?;
    if crate::uncensor::running() {
        return Err(crate::uncensor::BUSY.into());
    }
    if let Some(id) = req.music.as_ref().and_then(|m| m.custom.as_deref()).filter(|c| !c.is_empty() && req.op == "music") {
        train::get(&state.db.lock().unwrap(), &state.work_cipher()?, id).ok_or("That custom model is no longer here.")?;
        if !installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == train::BASE_MODEL) {
            return Err("Custom models play on ACE-Step 1.5 Base: install it from Models first.".into());
        }
    }
    if req.op == "studio_part" && req.music.as_ref().and_then(|m| m.track.as_deref()).is_none_or(|t| !music::PARTS.contains(&t)) {
        return Err(format!("Pick the part to make ({}).", music::PARTS.join(", ").replace('_', " ")));
    }
    if req.op == "train" || req.op == "trainer" {
        let t = req.train.clone().unwrap_or_default();
        if req.op == "train" && t.name.trim().is_empty() {
            return Err("Give the custom model a name.".into());
        }
        if req.op == "train" && t.files.len() + t.songs.len() == 0 && t.voice.as_deref().is_none_or(str::is_empty) {
            return Err("Pick at least one of your songs, or a voice with singing (3 or more songs teach its sound, 10 or more teach it best).".into());
        }
        if state.catalog.media.trainer.is_none() || !cfg!(windows) {
            return Err("Training isn't available on this system yet.".into());
        }
        if !crate::natural::nvidia_here(state) {
            return Err("Training needs an NVIDIA graphics card (8 GB or more).".into());
        }
    }
    if req.op == "talk" {
        if state.catalog.media.ditto.is_none() || !cfg!(windows) {
            return Err("Quick talking pictures aren't available on this system yet.".into());
        }
        if !crate::natural::nvidia_here(state) {
            return Err("The quick way needs an NVIDIA graphics card. The detailed way (Wan 2.2 S2V) works on others.".into());
        }
        if req.source.is_none() {
            return Err("Pick a picture to work on first.".into());
        }
    }
    if let Some(id) = cloud_pick(state, req) {
        let c = crate::cloud::media_choices(&state.db.lock().unwrap()).into_iter().find(|c| c.id == id);
        match c {
            Some(c) if !cloud_can(&c, &req.op) => return Err(format!("{} can't do that kind of job.", c.name)),
            None => return Err("That cloud model isn't available: it needs the Cloud level, the provider turned on and its key.".into()),
            _ => {}
        }
    } else if !no_model(&req.op) {
        choose(state, model_op(req), req.model_id.as_deref(), req.source.is_some())?;
    }
    // Podcasts and readings check their speakers and voices themselves.
    if matches!(req.op.as_str(), "podcast" | "dialogue") {
        return podcast::check(state, req);
    }
    if req.op == "explainer" {
        if !cfg!(windows) {
            return Err("Explainer videos aren't available on this system yet.".into());
        }
        if req.prompt.trim().is_empty() && req.talk.as_ref().is_none_or(|t| t.files.is_empty()) {
            return Err("Say what the video explains, or add files.".into());
        }
        if let Some(id) = recorded_voice(req) {
            let v = own_voice::get(&state.db.lock().unwrap(), &state.work_cipher()?, id).ok_or("That recorded voice is no longer here.")?;
            if !v.can_speak() {
                return Err(format!("Read at least three lines for {} in Studio › Your voice first.", v.name));
            }
        }
        return Ok(());
    }
    if matches!(req.op.as_str(), "listen" | "remake" | "map" | "revoice" | "cover_art" | "stems" | "dry" | "song_video" | "studio_part") && req.source.is_none() {
        return Err("Pick a song first.".into());
    }
    // A recorded voice: one that's here and has what the job needs.
    if let Some(id) = recorded_voice(req) {
        let v = own_voice::get(&state.db.lock().unwrap(), &state.work_cipher()?, id).ok_or("That recorded voice is no longer here.")?;
        if req.op == "narrate" && !v.can_speak() {
            return Err(format!("Read at least three lines for {} in Studio › Your voice first.", v.name));
        }
        if req.op != "narrate" && !v.can_sing() {
            return Err(format!("Record a little of {} in Studio › Your voice first.", v.name));
        }
    } else if req.op == "revoice" {
        return Err("Pick whose voice sings it.".into());
    }
    let needs_source = matches!(req.op.as_str(), "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background");
    if needs_source && req.source.is_none() {
        return Err("Pick a picture to work on first.".into());
    }
    // Work on a song the person has: the song, and for some the part.
    let task = req.music.as_ref().and_then(|m| m.task.as_deref()).filter(|t| *t != "new");
    if req.op == "music" && task.is_some() {
        let m = req.music.as_ref().map(|m| m.clone()).unwrap_or_default();
        if m.audio.is_none() {
            return Err("Pick the song to work on first.".into());
        }
        if matches!(task, Some("add" | "extract")) && m.track.as_deref().is_none_or(|t| t.trim().is_empty()) {
            return Err("Pick the part (vocals, drums, guitar…).".into());
        }
        if let Some(t) = m.track.as_deref().map(|t| t.trim().to_lowercase().replace(' ', "_")).filter(|t| !t.is_empty()) {
            if !music::PARTS.contains(&t.as_str()) {
                return Err(format!("The music model knows these parts: {}.", music::PARTS.join(", ").replace('_', " ")));
            }
        }
        if !matches!(task, Some("cover" | "repaint" | "extend" | "add" | "extract" | "complete")) {
            return Err("Unknown kind of change to a song.".into());
        }
    }
    let described = req.music.as_ref().is_some_and(MusicSpec::describes);
    let needs_prompt = matches!(req.op.as_str(), "generate" | "edit" | "restyle" | "video" | "music" | "sound" | "narrate")
        && !(req.op == "music" && (described || matches!(task, Some("repaint" | "extend" | "add" | "extract" | "complete"))));
    if needs_prompt && req.prompt.trim().is_empty() && !(req.op == "video" && req.source.is_some()) {
        return Err(if req.op == "narrate" { "Type the text to read aloud." } else { "Describe what you'd like first." }.into());
    }
    if req.op == "fill" && req.mask.is_none() {
        return Err("Paint over the part to change first.".into());
    }
    if let Some(w) = &req.wallpaper {
        w.checked()?;
        if !matches!(req.op.as_str(), "generate" | "video") {
            return Err("Wallpapers are new pictures or clips.".into());
        }
    }
    Ok(())
}

/// Queues a job and runs it when it's its turn. Returns what it made.
pub async fn run(state: &Arc<AppState>, id: String, req: Request) -> Result<Vec<MediaItem>, String> {
    // Built on the heap: the job's future is large, and awaited on a 2 MB
    // worker thread (or a test's) it overflowed the stack.
    Box::pin(run_job_now(state, id, req)).await
}

async fn run_job_now(state: &Arc<AppState>, id: String, req: Request) -> Result<Vec<MediaItem>, String> {
    if crate::sync::compute::media_here(state, &req) {
        return on_compute(state, id, req).await;
    }
    // A custom model plays on the model it was trained on.
    let req = match req.music.as_ref().and_then(|m| m.custom.as_deref()).filter(|c| !c.is_empty()) {
        Some(_) if req.op == "music" => Request { model_id: Some(train::BASE_MODEL.into()), ..req },
        _ => req,
    };
    check(state, &req)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let on_cloud = cloud_pick(state, &req);
    let model = if no_model(&req.op) || on_cloud.is_some() { None } else { choose(state, model_op(&req), req.model_id.as_deref(), req.source.is_some()).ok() };
    if let Some(m) = &model {
        files_here(state, m)?;
    }
    let est = match (&model, &on_cloud) {
        (Some(m), _) => estimate(state, m, &req),
        (None, Some(_)) if req.op == "video" => 180.0,
        _ if req.op == "train" => train_estimate(state, &req),
        _ if req.op == "trainer" => 900.0,
        _ if req.op == "talk" => ditto::estimate(req.seconds.unwrap_or(15.0) as f64, state.catalog.media.ditto.as_ref().is_some_and(|d| ditto::ready(&state.paths, d))),
        // Writing, then reading about as fast as it plays.
        _ if req.op == "podcast" => 60.0 + 50.0 * req.talk.as_ref().and_then(|t| t.minutes).unwrap_or(5.0) as f64,
        _ if req.op == "dialogue" => 10.0 + req.prompt.len() as f64 / 40.0,
        _ if req.op == "explainer" => 60.0 + 40.0 * req.talk.as_ref().and_then(|t| t.minutes).unwrap_or(3.0) as f64,
        (None, Some(_)) => 20.0,
        _ => 10.0,
    };
    {
        let info = JobInfo {
            id: id.clone(),
            op: req.op.clone(),
            prompt: req.prompt.chars().take(200).collect(),
            model_id: model.as_ref().map(|m| m.id.clone()).or_else(|| on_cloud.clone()),
            status: "queued".into(),
            stage: "Waiting for the job before it".into(),
            fraction: 0.0,
            eta_secs: None,
            est_secs: est,
            chat_id: req.chat_id.clone(),
            on: None,
            device: None,
            gpu_usage: None,
            gpu_temp: None,
            cpu_usage: None,
            ram_usage: None,
            gpu_mem: None,
        };
        jobs().lock().unwrap().push(Job { info: info.clone(), cancel: cancel.clone() });
        emit(state, "media:progress", json!(info));
    }
    struct Done(String);
    impl Drop for Done {
        fn drop(&mut self) {
            jobs().lock().unwrap().retain(|j| j.info.id != self.0);
        }
    }
    let _done = Done(id.clone());
    // Which card is which, so every media engine runs on the dedicated one
    // (after the job shows, as it can take a few seconds after a start).
    crate::engine::learn_devices(state).await;

    // Its turn for the graphics card (`lineup`): after what's ahead of it,
    // or now when it's part of a chat's reply. Taking it out of the line,
    // or Stop, ends the wait (a chat's Stop hung until the job before
    // finished). Cloud jobs don't use the card.
    let entry = crate::lineup::Entry::studio(&id, if req.prompt.trim().is_empty() { op_what(&req.op) } else { req.prompt.trim() }, op_what(&req.op), card_need(state, &req, model.as_ref()))
        .second_ok(matches!(req.op.as_str(), "map" | "stems" | "dry" | "revoice"))
        .processor_ok(on_the_processor_too(&req.op));
    let turn = match &on_cloud {
        Some(_) => None,
        None => match req.chat_id.as_deref().and_then(|c| crate::lineup::inside_reply(state, c, entry.clone())) {
            Some(t) => Some(t),
            None => Some(
                crate::lineup::wait(state, entry, cancel.clone(), |place| {
                    update(state, &id, |i| i.stage = if place <= 1 { "Next in line for the graphics card".into() } else { format!("Waiting in line for the graphics card ({place} in line)") });
                })
                .await?,
            ),
        },
    };
    let (second_card, forced_processor) = match turn.as_ref().map(|t| t.card) {
        Some(Some(crate::lineup::Card::Second)) => (true, false),
        Some(None) => (false, true),
        _ => (false, false),
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let started = Instant::now();
    update(state, &id, |i| {
        i.status = "running".into();
        i.stage = "Starting".into();
    });
    // What makes it: the dedicated card for pictures and video (and music,
    // with the card's music engine), else the processor, or a cloud model.
    let card_engine = matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "video" | "speak" | "talk" | "tune" | "coach" | "cover_art")
        || (matches!(req.op.as_str(), "music" | "sound" | "studio_part" | "video_sound") && music_engine_dir(state).is_some_and(|d| d.to_string_lossy().contains("vulkan")))
        // HeartMuLa only runs on a card.
        || (req.op == "music" && model.as_ref().is_some_and(|m| m.file("heartmula").is_some()))
        // A song's parts taken apart and read, or its voice sung again, on a
        // card (CUDA or DirectML).
        || (matches!(req.op.as_str(), "map" | "stems" | "dry" | "revoice" | "dub") && crate::natural::card_here(state))
        // Read in a recorded or made voice: its engine's card build.
        || (matches!(req.op.as_str(), "narrate" | "podcast" | "dialogue" | "explainer") && recorded_voice(&req).is_some());
    let (gpu_name, cpu_name) = {
        let hw = state.hardware.read().unwrap();
        (hw.best_gpu().map(|g| g.name.clone()), hw.cpu_name.clone())
    };
    let has_card = state.budget().vram > 0 && gpu_name.is_some();
    // Meant for the card, with none to use: said why, and asked first.
    if on_cloud.is_none() && card_engine && !has_card && !forced_processor {
        let why = if gpu_name.is_none() { crate::natural::no_card(state) } else { "SulcusAI's limit for the graphics card is set to none (Settings › This PC)".to_string() };
        let ask_progress = |stage: &str, f: f64| update(state, &id, |i| {
            i.stage = stage.to_string();
            i.fraction = f;
        });
        crate::cardfirst::processor_ok(state, Some(&id), op_what(&req.op), &why, &cancel, &ask_progress).await?;
    }
    let on_processor = Arc::new(AtomicBool::new(!(card_engine && has_card) || forced_processor));
    let device_now = {
        let (on_processor, gpu_name, cpu_name, cloud) = (on_processor.clone(), gpu_name.clone(), cpu_name.clone(), on_cloud.clone());
        move || -> (String, Option<String>) {
            match &cloud {
                Some(c) => ("cloud".into(), Some(cloud_label(c))),
                None if on_processor.load(Ordering::Relaxed) => ("processor".into(), (!cpu_name.is_empty()).then(|| cpu_name.clone())),
                None => ("graphics card".into(), gpu_name.clone()),
            }
        }
    };
    {
        let (on, device) = device_now();
        update(state, &id, |i| {
            i.on = Some(on);
            i.device = device;
        });
    }
    let progress = |stage: &str, fraction: f64| {
        if stage == PROCESSOR_STAGE {
            on_processor.store(true, Ordering::Relaxed);
        }
        let elapsed = started.elapsed().as_secs_f64();
        let eta = if fraction > 0.12 { elapsed / fraction - elapsed } else { (est - elapsed).max(5.0) };
        update(state, &id, |i| {
            i.stage = stage.to_string();
            i.fraction = fraction.clamp(0.0, 1.0);
            i.eta_secs = Some(eta.max(0.0).round());
        });
    };
    // The card and processor, every 2 seconds while it works: shown live,
    // and the peak temperature and average use kept with what's made.
    let watching = Arc::new(AtomicBool::new(true));
    let readings: Arc<std::sync::Mutex<Vec<Reading>>> = Arc::default();
    if on_cloud.is_none() {
        let (st, id2, w, r, device_now) = (state.clone(), id.clone(), watching.clone(), readings.clone(), device_now.clone());
        tauri::async_runtime::spawn(async move {
            crate::perf::cpu_usage();
            while w.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let g = if has_card { crate::perf::gpu_reading(&st).await } else { Default::default() };
                let cpu = tokio::task::spawn_blocking(crate::perf::cpu_usage).await.ok();
                let ram = {
                    let mut sys = sysinfo::System::new();
                    sys.refresh_memory();
                    (sys.total_memory() > 0).then(|| sys.used_memory() as f32 * 100.0 / sys.total_memory() as f32)
                };
                let gpu_mem = match (g.mem_used, g.mem_total) {
                    (Some(u), Some(t)) if t > 0 => Some(u as f32 * 100.0 / t as f32),
                    _ => None,
                };
                let (on, device) = device_now();
                r.lock().unwrap().push(Reading { gpu_usage: g.usage, gpu_temp: g.temp, cpu_usage: cpu, on_card: on == "graphics card" });
                update(&st, &id2, |i| {
                    i.gpu_usage = g.usage;
                    i.gpu_temp = g.temp;
                    i.cpu_usage = cpu;
                    i.ram_usage = ram;
                    i.gpu_mem = gpu_mem;
                    i.on = Some(on);
                    i.device = device;
                });
            }
        });
    }
    let result = match &on_cloud {
        Some(cid) => {
            let ctx = RunCtx { state, req: &req, cancel: &cancel, progress: &progress, reserved: 0 };
            match crate::cloud::media_target(state, cid, &state.work_cipher()?) {
                Ok(t) => cloud::run(&ctx, cid, &t).await,
                Err(e) => Err(e),
            }
        }
        // Stop reaches the helper models too (planning, checking, writing).
        // And Stop always stops: whatever the job is waiting on (lyrics from
        // a chat model, a download, an engine that hung) is let go, and the
        // engines it started end with it (a song stuck "making" ignored Stop).
        None => {
            let work = JOB_ID.scope(id.clone(), SECOND_CARD.scope(second_card, ON_PROCESSOR.scope(forced_processor, crate::vision::JOB_STOP.scope(cancel.clone(), Box::pin(run_job(state, &id, &req, model.as_ref(), &cancel, &progress))))));
            tokio::pin!(work);
            loop {
                tokio::select! {
                    r = &mut work => break r,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {
                        if cancel.load(Ordering::Relaxed) {
                            // A moment to stop by itself first (it keeps what it can).
                            match tokio::time::timeout(std::time::Duration::from_secs(3), &mut work).await {
                                Ok(r) => break r,
                                Err(_) => {
                                    crate::applog::line("media", &format!("Stopped a job that didn't stop by itself ({})", req.op));
                                    break Err(download::CANCELLED.into());
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    if let (Ok(items), Some(m)) = (&result, &model) {
        if !items.is_empty() {
            learn_speed(state, m, &req, started.elapsed().as_secs_f64());
            crate::cleanup::touch_model(state, &m.id);
        }
    }
    // A saved artist's first new song becomes their voice for the next ones.
    let artist_of = req.music.as_ref().filter(|m| matches!(m.task.as_deref(), None | Some("new"))).and_then(|m| m.persona.clone());
    if let (Ok(items), Some(name), Ok(c)) = (&result, artist_of, state.work_cipher()) {
        if let Some(first) = items.first().filter(|i| i.kind == "audio") {
            let conn = state.db.lock().unwrap();
            if let Some(p) = persona::find(&conn, &c, &name) {
                if persona::first_voice(&conn, &c, &p.id, &first.id, first.seed, |id| store::get(&conn, &c, id).is_some()).unwrap_or(false) {
                    crate::applog::line("media", &format!("{}'s first song is their voice now", p.name));
                }
            }
        }
    }
    let name = match req.op.as_str() {
        "video" => "a video",
        "speak" | "talk" => "a talking picture",
        "music" => "a song",
        "sound" => "a sound",
        "narrate" => "a narration",
        "podcast" => "a podcast",
        "dialogue" => "a reading",
        "explainer" => "an explainer video",
        "video_edit" => "an edited video",
        "video_sound" => "a video with sound",
        "dub" => "a dubbed video",
        "captions" => "a captioned video",
        _ => "a picture",
    };
    watching.store(false, Ordering::Relaxed);
    let took = started.elapsed().as_secs_f64();
    let with = model.as_ref().map(|m| m.id.clone()).or_else(|| on_cloud.clone()).unwrap_or_else(|| if req.op == "video_sound" { "the music model".into() } else if matches!(req.op.as_str(), "video_edit" | "dub" | "captions") { "FFmpeg".into() } else { "the built-in voice".into() });
    // What making it took, kept with each item (and shown under it).
    let result = result.map(|mut items| {
        let r = readings.lock().unwrap();
        let picture_or_video = matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "video");
        let steps = picture_or_video.then(|| req.steps.or(model.as_ref().map(|m| m.defaults.steps)).unwrap_or(0)).filter(|s| *s > 0);
        let (on, device) = device_now();
        let on_card = on == "graphics card";
        // The readings from while the one that made it was working.
        let mine: Vec<&Reading> = r.iter().filter(|x| x.on_card == on_card).collect();
        let avg = |v: Vec<f32>| (!v.is_empty()).then(|| (v.iter().sum::<f32>() / v.len() as f32).round());
        let peak = |v: Vec<f32>| v.into_iter().reduce(f32::max).map(f32::round);
        let gpu = |f: fn(&Reading) -> Option<f32>| if on_card { mine.iter().filter_map(|x| f(x)).collect() } else { Vec::new() };
        let cpu: Vec<f32> = if on == "processor" { mine.iter().filter_map(|x| x.cpu_usage).collect() } else { Vec::new() };
        let stats = store::MadeStats {
            secs: (took * 10.0).round() / 10.0,
            on,
            device,
            steps,
            prompt_tokens: (!req.prompt.trim().is_empty()).then(|| approx_tokens(&req.prompt)),
            audio_tokens: None,
            frames: None,
            gpu_peak_temp: peak(gpu(|x| x.gpu_temp)),
            gpu_avg_usage: avg(gpu(|x| x.gpu_usage)),
            gpu_peak_usage: peak(gpu(|x| x.gpu_usage)),
            cpu_avg_usage: avg(cpu.clone()),
            cpu_peak_usage: peak(cpu),
        };
        if let Ok(c) = state.work_cipher() {
            let conn = state.db.lock().unwrap();
            for item in items.iter_mut() {
                let mut s = stats.clone();
                if item.kind == "audio" && matches!(req.op.as_str(), "music" | "sound") {
                    s.audio_tokens = Some((item.seconds * 5.0).round() as u32);
                }
                if item.kind == "video" {
                    s.frames = Some((item.seconds * model.as_ref().map_or(24, |m| m.defaults.fps.max(1)) as f64).round() as u32);
                }
                item.stats = Some(s);
                let _ = store::update(&conn, &c, item);
            }
        }
        items
    });
    match &result {
        Ok(items) => {
            let how = items.first().and_then(|i| i.stats.as_ref()).map(|s| {
                let mut out = format!(" on the {}", s.on);
                if let Some(d) = &s.device {
                    out += &format!(" ({d})");
                }
                if let Some(t) = s.gpu_peak_temp {
                    out += &format!(", card up to {t:.0} °C");
                }
                if let Some(u) = s.gpu_avg_usage.or(s.cpu_avg_usage) {
                    out += &format!(", {u:.0}% busy on average");
                }
                out
            });
            crate::applog::line("media", &format!("{} with {with}: done in {took:.0} s{}", req.op, how.unwrap_or_default()))
        }
        Err(e) if e == download::CANCELLED => crate::applog::line("media", &format!("{} with {with}: stopped after {took:.0} s", req.op)),
        Err(e) => {
            crate::applog::line("media", &format!("{} with {with}: failed after {took:.0} s: {}", req.op, e.lines().take(6).collect::<Vec<_>>().join(" | ")));
            crate::devmode::soon(state);
        }
    }
    match &result {
        Ok(_) if on_cloud.is_some() => state.log("media", &format!("Made {name} with a cloud model")),
        Ok(_) => state.log("media", &format!("Made {name} on this PC")),
        Err(e) if e != download::CANCELLED => state.log("media", &format!("Making {name} failed: {}", e.lines().next().unwrap_or(""))),
        Err(_) => {}
    }
    result
}

/// How many "default jobs" this request is, for timing estimates.
fn job_scale(m: &MediaModel, req: &Request) -> f64 {
    match m.kind {
        Kind::Video => (req.seconds.unwrap_or(m.defaults.seconds).max(1.0) / m.defaults.seconds.max(1.0)) as f64,
        Kind::Music => (req.seconds.unwrap_or(m.defaults.seconds).max(10.0) / m.defaults.seconds.max(10.0)) as f64 * req.count.unwrap_or(1).clamp(1, 3) as f64,
        Kind::Image => req.count.unwrap_or(1).clamp(1, 4) as f64,
        _ => 1.0,
    }
}

fn estimate(state: &AppState, m: &MediaModel, req: &Request) -> f64 {
    let hw = state.hardware.read().unwrap().clone();
    let record = installed(&state.db.lock().unwrap()).into_iter().find(|i| i.model_id == m.id);
    if req.op == "edit" {
        if let Some(s) = record.as_ref().and_then(|i| i.edit_secs) {
            return s.max(3.0);
        }
    }
    let measured = record.and_then(|i| i.secs);
    let f = fit(m, &hw, &state.budget(), 0, measured);
    if req.op == "tune" {
        // About 27 practice pictures at 768 pixels, plus writing and checking them.
        return f.est_secs * 27.0 * 0.6 + 300.0;
    }
    if req.op == "coach" {
        // About 110 practice pictures and edit steps at 768 pixels, the
        // checking, and a training run.
        return f.est_secs * 110.0 * 0.6 + 1500.0;
    }
    // A checked edit not timed yet: about three plain pictures (plan, make, check).
    let edit = if req.op == "edit" { 3.0 } else { 1.0 };
    (f.est_secs * job_scale(m, req) * edit).max(3.0)
}

/// Keeps a running average of how long a default job takes here.
fn learn_speed(state: &AppState, m: &MediaModel, req: &Request, secs: f64) {
    let per = secs / job_scale(m, req).max(0.1);
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    if let Some(i) = list.iter_mut().find(|i| i.model_id == m.id) {
        let slot = if req.op == "edit" { &mut i.edit_secs } else { &mut i.secs };
        *slot = Some(match *slot {
            Some(old) => (old * 0.6 + per * 0.4).round(),
            None => per.round(),
        });
        let _ = save_installed(&conn, &list);
    }
}

pub fn cancel(id: &str) {
    if let Some(j) = jobs().lock().unwrap().iter().find(|j| j.info.id == id) {
        j.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn list_jobs() -> Vec<JobInfo> {
    jobs().lock().unwrap().iter().map(|j| j.info.clone()).collect()
}

/// A scratch folder for one job's files, removed when it's dropped.
struct Work(PathBuf);

impl Drop for Work {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// At startup: work folders a crash left behind (nothing is being made yet).
pub fn clean_work(paths: &Paths) {
    if let Ok(entries) = std::fs::read_dir(store::dir(paths).join("work")) {
        for e in entries.flatten() {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    // Half-written gallery files a crash left (written as .part, then named).
    if let Ok(entries) = std::fs::read_dir(store::dir(paths)) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "part") {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }
}

fn work_dir(paths: &Paths, id: &str) -> Result<Work, String> {
    let dir = store::dir(paths).join("work").join(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(Work(dir))
}

fn random_seed() -> i64 {
    (uuid::Uuid::new_v4().as_u128() & 0x7fff_ffff) as i64
}

struct RunCtx<'a> {
    state: &'a Arc<AppState>,
    req: &'a Request,
    cancel: &'a AtomicBool,
    progress: &'a (dyn Fn(&str, f64) + Sync),
    /// Graphics memory the loaded chat model holds.
    reserved: u64,
}

/// Graphics memory the loaded chat model holds, estimated the way the
/// catalog plans it. The engines can't see it reliably themselves: they
/// read the card as nearly empty and run out partway through.
pub(crate) async fn chat_model_vram(state: &Arc<AppState>) -> u64 {
    let (st, sees) = {
        let mut e = state.engine.lock().await;
        (e.status(), e.sees())
    };
    let (Some(id), Some(quant), Some(ctx), Some(layers)) = (st.model_id, st.quant, st.ctx, st.gpu_layers) else { return 0 };
    let Some(spec) = state.model_spec(&id) else { return 0 };
    let Some(v) = spec.variant(&quant) else { return 0 };
    let share = (layers as f64 / (spec.arch.n_layer + 1) as f64).min(1.0);
    let kv = crate::catalog::kv_cache_bytes(&spec.arch, ctx);
    let encoder = if sees { spec.vision.as_ref().map_or(0, |v| v.size) } else { 0 };
    ((v.size + kv) as f64 * share) as u64 + encoder + 700 * 1024 * 1024
}

/// The graphics card ran short or its driver failed (as opposed to, say, a
/// broken model file, which another try wouldn't fix).
fn card_trouble(e: &str) -> bool {
    let l = e.to_lowercase();
    ["make enough memory", "compute failed", "out of memory", "vulkan", "device lost", "vk_error", "ggml_backend"].iter().any(|w| l.contains(w))
}

/// One look at the card and processor while a job runs.
struct Reading {
    gpu_usage: Option<f32>,
    gpu_temp: Option<f32>,
    cpu_usage: Option<f32>,
    /// The card was making it then (not the processor).
    on_card: bool,
}

/// "gpt-image-1 (openai)" from "cloud:openai:gpt-image-1".
fn cloud_label(id: &str) -> String {
    let mut parts = id.splitn(3, ':').skip(1);
    match (parts.next(), parts.next()) {
        (Some(provider), Some(model)) => format!("{model} ({provider})"),
        _ => id.to_string(),
    }
}

/// What progress says when a picture moved to the processor.
pub const PROCESSOR_STAGE: &str = "The graphics card couldn't make it, so it's being made on the processor (slower)";

/// About how many tokens a description is for a model's text reader (words
/// and punctuation, the way most tokenizers split English).
fn approx_tokens(text: &str) -> u32 {
    let words = text.split_whitespace().count() as f64;
    let marks = text.chars().filter(|c| c.is_ascii_punctuation()).count() as f64;
    (words * 1.3 + marks * 0.5).round().max(1.0) as u32
}

/// The value after a flag, if it's there.
fn flag_value<'a>(args: &'a [std::ffi::OsString], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.to_str())
}

/// `flag value` set (replaced, or put before the output path, which stays last).
fn with_flag(args: &[std::ffi::OsString], flag: &str, value: Option<&str>) -> Vec<std::ffi::OsString> {
    let mut out = args.to_vec();
    if let Some(i) = out.iter().position(|a| a == flag) {
        match value {
            Some(v) if i + 1 < out.len() => out[i + 1] = v.into(),
            _ => {}
        }
        return out;
    }
    let at = out.iter().position(|a| a == "-o").unwrap_or(out.len());
    let mut add: Vec<std::ffi::OsString> = vec![flag.into()];
    if let Some(v) = value {
        add.push(v.into());
    }
    out.splice(at..at, add);
    out
}

/// A job planned to stay on the graphics card: 1 GiB of it kept free for
/// spikes and other programs (unless a budget is set already), and the
/// picture decoded in tiles on cards under 12 GB or for big pictures.
fn card_friendly(args: &[std::ffi::OsString], vram_gib: f64) -> Vec<std::ffi::OsString> {
    let mut out = if flag_value(args, "--max-vram").is_some() || streamed_only(args) { args.to_vec() } else { with_flag(args, "--max-vram", Some("-1")) };
    let pixels = flag_value(args, "-W").and_then(|w| w.parse::<f64>().ok()).unwrap_or(0.0) * flag_value(args, "-H").and_then(|h| h.parse::<f64>().ok()).unwrap_or(0.0);
    if vram_gib < 12.0 || pixels > 1.2e6 {
        out = with_flag(&out, "--vae-tiling", None);
    }
    // Video: the weights wait in memory and go to the card only while
    // they're used. Kept on the card, Wan's 7 GB left its frame decoder
    // 2.6 GB of a 12 GB card, and decoding in tiny tiles took 19 minutes.
    if vram_gib < 20.0 && args.iter().any(|a| a == "vid_gen") {
        out = with_flag(&out, "--offload-to-cpu", None);
    }
    out
}

/// Wan 2.2 S2V (a clip moved by a voice): its 14B weights only stream to the
/// card. The engine's own splitting on top of that kept each part's output
/// on the card too, and a 12 GB card ran out partway through a 5 s clip.
fn streamed_only(args: &[std::ffi::OsString]) -> bool {
    args.iter().any(|a| a == "--audio")
}

/// After the card failed: more of it left free (or half the budget), the
/// text reader on the processor, and no fast attention. With the card
/// `pinned` (the only one the engine sees), the rest is named for it, since
/// the engine's own placement otherwise may move it to integrated graphics.
fn lighter_on_card(args: &[std::ffi::OsString], pinned: bool) -> Vec<std::ffi::OsString> {
    let budget = flag_value(args, "--max-vram").and_then(|v| v.parse::<f64>().ok()).unwrap_or(-1.0);
    let lighter = if budget > 0.0 { format!("{:.1}", (budget / 2.0).max(1.0)) } else { "-3".to_string() };
    let capped = if streamed_only(args) { args.to_vec() } else { with_flag(args, "--max-vram", Some(&lighter)) };
    // LTX keeps fast attention: without it its frames came out white.
    let ltx = args.iter().any(|a| a == "--audio-vae");
    let mut out: Vec<std::ffi::OsString> = capped.into_iter().filter(|a| ltx || a != "--diffusion-fa").collect();
    out = with_flag(&out, "--vae-tiling", None);
    if pinned && flag_value(&out, "--backend").is_none() {
        out = with_flag(&out, "--backend", Some("diffusion=Vulkan0,te=cpu,vae=Vulkan0"));
    }
    out
}

/// The argument list with the picture's size brought down to `side` pixels
/// on its longer edge (multiples of 16, as the engine needs).
fn smaller(args: &[std::ffi::OsString], side: u32) -> Vec<std::ffi::OsString> {
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.to_str()?.parse::<u32>().ok());
    let (Some(w), Some(h)) = (value("-W"), value("-H")) else { return args.to_vec() };
    if w.max(h) <= side {
        return args.to_vec();
    }
    let scale = side as f64 / w.max(h) as f64;
    let fit = |v: u32| (((v as f64 * scale) / 16.0).round() as u32 * 16).max(256);
    let (nw, nh) = (fit(w), fit(h));
    let mut out = args.to_vec();
    for (flag, v) in [("-W", nw), ("-H", nh)] {
        if let Some(i) = out.iter().position(|a| a == flag) {
            out[i + 1] = v.to_string().into();
        }
    }
    out
}

/// The argument list without a graphics-memory budget (for the processor build).
fn without_vram(args: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a == "--max-vram" {
            skip = true;
        } else {
            out.push(a.clone());
        }
    }
    out
}


impl RunCtx<'_> {
    fn limits(&self) -> sd::Limits {
        let l = self.state.limits();
        let b = self.state.budget();
        let vram = l.vram_bytes.min(b.vram_card).min(b.vram_card.saturating_sub(self.reserved));
        sd::Limits {
            threads: l.threads,
            // Only cap it when the performance mode or a loaded chat model leaves less than the card has.
            max_vram_gib: (b.vram_card > 0 && vram < b.vram_card).then(|| vram as f64 / GIB as f64),
        }
    }

    fn files(&self, m: &MediaModel) -> Vec<(String, PathBuf)> {
        m.files.iter().map(|f| (f.role.clone(), file_path(&self.state.paths, m, f))).collect()
    }

    async fn run_sd(&self, args: Vec<std::ffi::OsString>, work: &std::path::Path) -> Result<(), String> {
        let vram_gib = self.state.budget().vram_card as f64 / GIB as f64;
        let on_card = vram_gib > 0.0;
        // Started now on the processor while the card is busy: its build, smaller.
        if on_card && ON_PROCESSOR.try_with(|p| *p).unwrap_or(false) {
            let cpu = self.cpu_engine().await?;
            return self.run_sd_once(smaller(&without_vram(&args), 640), work, Some(cpu)).await;
        }
        if !on_card {
            return self.run_sd_once(args, work, None).await;
        }
        // Kept on the dedicated card: on a PC with integrated graphics too, the
        // engine would otherwise use whichever card it lists first.
        let pinned = crate::engine::learn_devices(self.state).await;
        // Planned to stay on the card: headroom it keeps free, and the
        // picture decoded in tiles where the last step's burst could overflow it.
        let mut first = card_friendly(&args, vram_gib);
        // LTX-2.3 (22B, with Gemma 3 12B reading the prompt) on a card under
        // 20 GB starts the lighter way: on a 12 GB card the first way ran out
        // partway after 5 minutes, and the lighter one made 5 s in 526 s.
        if vram_gib < 20.0 && args.iter().any(|a| a == "--audio-vae") {
            first = lighter_on_card(&first, pinned);
        }
        let mut last = match self.run_sd_once(first.clone(), work, None).await {
            Err(e) if card_trouble(&e) => e,
            other => return other,
        };
        // Still on the card, lighter each time: more headroom, the text reader
        // on the processor, no fast attention (some drivers fail with it),
        // then a smaller picture.
        let lighter = lighter_on_card(&first, pinned);
        let tries = [(lighter.clone(), "with more of its memory left free"), (smaller(&lighter, 768), "smaller")];
        for (try_args, how) in tries {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(download::CANCELLED.into());
            }
            crate::applog::line("media", &format!("The graphics card failed ({}); trying again on it {how}", last.lines().take(3).collect::<Vec<_>>().join(" | ")));
            (self.progress)("The graphics card ran short, so it's trying again with less of its memory", 0.0);
            match self.run_sd_once(try_args, work, None).await {
                Err(e) if card_trouble(&e) => last = e,
                other => return other,
            }
        }
        // The card failed three times: as a last resort, a small picture on
        // the processor (slow, but it doesn't depend on the card). Not videos,
        // which would take hours there.
        if self.cancel.load(Ordering::Relaxed) || !args.iter().any(|a| a == "img_gen") {
            return Err(last);
        }
        let why = format!("the graphics card failed three times ({})", last.lines().next().unwrap_or("").trim());
        let job = JOB_ID.try_with(|j| j.clone()).ok();
        if crate::cardfirst::processor_ok(self.state, job.as_deref(), "Making the picture", &why, self.cancel, self.progress).await? == crate::cardfirst::Go::Again {
            // The card was freed: once more on it before the processor.
            match self.run_sd_once(lighter.clone(), work, None).await {
                Err(e) if card_trouble(&e) => last = e,
                other => return other,
            }
        }
        crate::applog::line("media", &format!("The graphics card failed three times ({}); making the picture on the processor, smaller", last.lines().take(3).collect::<Vec<_>>().join(" | ")));
        (self.progress)(PROCESSOR_STAGE, 0.0);
        let cpu = self.cpu_engine().await.map_err(|why| format!("{last}\n\nCouldn't switch to the processor: {why}"))?;
        // At full size the processor takes many minutes; 640 pixels keeps it to a few.
        self.run_sd_once(smaller(&without_vram(&args), 640), work, Some(cpu)).await.map_err(|on_cpu| format!("{last}\n\nOn the processor too: {on_cpu}"))
    }

    /// The processor build of the picture engine, downloaded the first time it's needed (about 18 MB).
    async fn cpu_engine(&self) -> Result<PathBuf, String> {
        let c = &self.state.catalog.media;
        let (key, asset) = [engine::cpu_key(), "cpu-x64", "linux-cpu-x64"]
            .into_iter()
            .find_map(|k| c.engine.assets.get(k).map(|a| (k, a)))
            .ok_or("There's no processor build of the picture engine for this system.")?;
        let dir = sd_dir(c, key);
        if let Some(exe) = engine::find_exe(&self.state.paths.engines.join(&dir), SD_EXE) {
            return Ok(exe);
        }
        self.state.log("network", &format!("Downloading the stable-diffusion.cpp engine ({key}) from {}", crate::host_of(&asset.url)));
        engine::ensure_unpacked(&self.state.paths, &dir, asset, SD_EXE, self.cancel, |_, _| {}).await
    }

    async fn run_sd_once(&self, args: Vec<std::ffi::OsString>, work: &std::path::Path, exe: Option<PathBuf>) -> Result<(), String> {
        let exe = match exe {
            Some(e) => e,
            None => sd_exe(self.state)?,
        };
        let mut t = sd::Tracker::default();
        (self.progress)(t.stage.label(), t.fraction());
        proc::run(
            proc::Run {
                exe: &exe,
                args,
                cwd: work,
                log: &self.state.paths.logs.join("media.log"),
                low_priority: self.state.limits().low_priority,
                job: self.state.job(),
            },
            self.cancel,
            |l| {
                if t.line(l) {
                    (self.progress)(t.stage.label(), t.fraction());
                }
            },
        )
        .await
    }

    /// A practice picture for prompt tuning: square, 768 pixels, not kept.
    async fn render(&self, m: &MediaModel, prompt: &str, negative: &str, seed: i64, dir: &std::path::Path) -> Result<image::DynamicImage, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let d = &m.defaults;
        let side = if d.align > 16 { (768 / d.align) * d.align } else { 768 };
        let out = dir.join("out_%d.png");
        let args = sd::picture_args(
            &self.files(m),
            d,
            &sd::Picture {
                prompt,
                negative: Some(negative).filter(|n| !n.trim().is_empty()).or((!d.negative.is_empty()).then_some(d.negative.as_str())),
                width: side,
                height: side,
                steps: d.steps.clamp(1, 60),
                seed,
                count: 1,
                init: None,
                mask: None,
                refs: vec![],
                strength: None,
                out: &out,
            },
            &self.limits(),
        );
        self.run_sd(args, dir).await?;
        let made = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("out_")));
        imaging::decode(&std::fs::read(made.ok_or("The picture engine made nothing.")?).map_err(|e| e.to_string())?)
    }

    /// The request's example pictures (at most four).
    fn examples(&self) -> Result<Vec<image::DynamicImage>, String> {
        if self.req.examples.is_empty() {
            return Ok(Vec::new());
        }
        let c = self.state.work_cipher()?;
        Ok(self
            .req
            .examples
            .iter()
            .take(4)
            .filter_map(|id| store::read(&self.state.paths, &c, id).ok())
            .filter_map(|b| imaging::decode(&b).ok())
            .collect())
    }

    fn source(&self) -> Result<(MediaItem, image::DynamicImage), String> {
        let id = self.req.source.as_deref().ok_or("Pick a picture to work on first.")?;
        let c = self.state.work_cipher()?;
        let item = store::get(&self.state.db.lock().unwrap(), &c, id).ok_or("That picture is no longer in the gallery.")?;
        if item.kind != "image" {
            return Err("That isn't a picture.".into());
        }
        let bytes = store::read(&self.state.paths, &c, id)?;
        let img = imaging::decode(&bytes)?;
        // A cut-out's see-through parts are black underneath, and the models
        // read them as black: they get a plain white background instead.
        // The upscaler keeps the transparency itself.
        let img = if matches!(self.req.op.as_str(), "upscale" | "remove_background") { img } else { imaging::flatten(&img, [255, 255, 255]) };
        Ok((item, img))
    }

    /// For a picture made by editing another (and that by editing another…),
    /// the first one in the chain: what was made or attached before any edit.
    /// None when the picture isn't an edit, or its original is gone.
    fn original_of(&self, item: &MediaItem) -> Result<Option<image::DynamicImage>, String> {
        let c = self.state.work_cipher()?;
        let edited = |i: &MediaItem| matches!(i.op.as_str(), "edit" | "restyle" | "fill" | "extend");
        if !edited(item) {
            return Ok(None);
        }
        let mut at = item.clone();
        for _ in 0..20 {
            let Some(parent) = at.parent.as_deref() else { break };
            let Some(p) = store::get(&self.state.db.lock().unwrap(), &c, parent) else { break };
            at = p;
            if !edited(&at) {
                break;
            }
        }
        if at.id == item.id || at.kind != "image" {
            return Ok(None);
        }
        Ok(store::read(&self.state.paths, &c, &at.id).ok().and_then(|b| imaging::decode(&b).ok()))
    }

    /// Saves a made picture into the gallery.
    fn keep_picture(&self, img: &image::DynamicImage, m: Option<&MediaModel>, parent: Option<&str>, seed: Option<i64>) -> Result<MediaItem, String> {
        self.keep_as(&self.req.op, img, m, parent, seed)
    }

    /// A picture enlarged to cover `w`×`h`: with the upscaler when one is
    /// installed and it's much smaller, else smoothly resized.
    async fn enlarge_for(&self, img: &image::DynamicImage, w: u32, h: u32, work: &std::path::Path) -> image::DynamicImage {
        let (iw, ih) = img.dimensions();
        let need = (w as f64 / iw as f64).max(h as f64 / ih as f64);
        if need <= 1.25 {
            return img.clone();
        }
        let Ok(up) = choose(self.state, "upscale", None, false) else { return img.clone() };
        (self.progress)("Enlarging it to your screen's size", 0.95);
        let (src, out) = (work.join("small.png"), work.join("big.png"));
        let Ok(png) = imaging::png(img) else { return img.clone() };
        if std::fs::write(&src, png).is_err() {
            return img.clone();
        }
        let args = sd::upscale_args(&self.files(&up), &src, &out, &self.limits());
        match self.run_sd(args, work).await {
            Ok(()) => std::fs::read(&out).ok().and_then(|b| imaging::decode(&b).ok()).unwrap_or_else(|| img.clone()),
            Err(e) => {
                crate::applog::line("media", &format!("The upscaler failed ({e}); resizing the wallpaper instead"));
                img.clone()
            }
        }
    }

    fn keep_as(&self, op: &str, img: &image::DynamicImage, m: Option<&MediaModel>, parent: Option<&str>, seed: Option<i64>) -> Result<MediaItem, String> {
        let c = self.state.work_cipher()?;
        let png = imaging::png(img)?;
        let thumb = imaging::thumbnail(img)?;
        let (w, h) = img.dimensions();
        let item = MediaItem {
            kind: "image".into(),
            op: op.to_string(),
            prompt: self.req.asked.as_deref().unwrap_or(&self.req.prompt).trim().to_string(),
            model_id: m.map(|m| m.id.clone()),
            mime: "image/png".into(),
            width: w,
            height: h,
            parent: parent.map(str::to_string),
            chat_id: self.req.chat_id.clone(),
            seed,
            ..Default::default()
        };
        store::save(&self.state.db.lock().unwrap(), &self.state.paths, &c, item, &png, Some(&thumb))
    }
}

/// Unloads the chat model when nothing is using it, so a picture or video
/// gets the whole graphics card.
///
/// For a job a chat asked for, whose reply goes on afterwards: the chat model
/// steps aside only when the picture model fits the card alone but not beside
/// it (a 14B chat model took 9 of 12 GB, and pictures fell back to the
/// processor: minutes instead of seconds). It's paused, keeping its address,
/// and `give_back` brings it back for the rest of the reply.
async fn make_room(state: &Arc<AppState>, req: &Request, model: Option<&MediaModel>) -> Option<crate::engine::Paused> {
    let vram = state.budget().vram_card;
    if vram == 0 {
        return None;
    }
    if req.chat_id.is_none() {
        if state.generations.lock().unwrap().is_empty() {
            let mut e = state.engine.lock().await;
            if e.status().model_id.is_some() {
                e.stop().await;
                emit(state, "engine:unloaded", json!({}));
            }
        }
        return None;
    }
    // Another chat replying on it too: it stays.
    if state.generations.lock().unwrap().len() > 1 {
        return None;
    }
    let need = (model?.min_vram_gb * GIB as f64) as u64;
    let reserved = chat_model_vram(state).await;
    if reserved == 0 || vram < need || vram.saturating_sub(reserved) >= need {
        return None;
    }
    let paused = state.engine.lock().await.pause().await?;
    crate::applog::line("media", &format!("Paused {} so the picture gets the graphics card", paused.model_id));
    emit(state, "engine:unloaded", json!({}));
    Some(paused)
}

/// Starts a model paused for a chat's picture again, at its old address, so
/// the chat's reply carries on.
pub(crate) async fn give_back(state: &Arc<AppState>, paused: crate::engine::Paused) {
    match crate::llm_endpoint(state, Some(paused.model_id.clone()), paused.vision, |_| {}).await {
        Ok(_) => crate::applog::line("media", &format!("Started {} again after the picture", paused.model_id)),
        Err(e) => crate::applog::line("media", &format!("Couldn't start {} again after the picture: {e}", paused.model_id)),
    }
}

async fn run_job(state: &Arc<AppState>, id: &str, req: &Request, model: Option<&MediaModel>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let music_on_gpu = matches!(req.op.as_str(), "music" | "sound" | "remake" | "studio_part") && (music_engine_dir(state).is_some_and(|d| d.to_string_lossy().contains("vulkan")) || model.is_some_and(|m| m.file("heartmula").is_some()));
    // A saved artist's sound and voice, under the song's own choices.
    let as_artist = with_persona(state, req)?;
    let req = as_artist.as_ref().unwrap_or(req);
    // An artist's style is read by the chat model too, before the lyrics.
    let styled = artist::resolve(state, req, progress).await;
    let req = styled.as_ref().unwrap_or(req);
    // Auto length: what's known before the lyrics are written.
    let timed = auto_seconds(req).map(|s| Request { seconds: Some(s), ..req.clone() });
    let req = timed.as_ref().unwrap_or(req);
    let sexed = with_own_voice_sex(state, req);
    let req = sexed.as_ref().unwrap_or(req);
    // Lyrics come from the chat model: written before the card is cleared for
    // the music engine, not after (that loaded the chat model again beside it).
    let mut with_lyrics;
    let req = if wants_written_lyrics(req) {
        progress("Writing lyrics", 0.02);
        let spec = req.music.clone().unwrap_or_default();
        // Auto with nothing to go on: the writer picks the sections, and the
        // song is as long as they need.
        let open = spec.auto_length && req.seconds.is_none() && matches!(spec.task.as_deref(), None | Some("new"));
        let seconds = if open { None } else { Some(req.seconds.unwrap_or(model.map_or(30.0, |m| m.defaults.seconds.max(30.0))).clamp(10.0, 600.0)) };
        with_lyrics = req.clone();
        let flow = spec.flow(&req.prompt);
        // Told the style, mood and voice too, not only the person's words.
        match lyrics::write(state, &song_caption(req), seconds, req.language.as_deref(), &spec.sections(), &spec.lyrics_guide(), flow).await {
            Ok(l) => {
                if open {
                    with_lyrics.seconds = Some(lyrics::length_of(&l, flow));
                }
                with_lyrics.lyrics = Some(l);
            }
            Err(why) => {
                // Nothing written here: the music model writes its own.
                crate::applog::line("media", &format!("Couldn't write lyrics before the song ({why:?}); the music model writes them"));
                with_lyrics.lyrics = None;
                with_lyrics.lyrics_problem = Some(why);
            }
        }
        &with_lyrics
    } else {
        req
    };
    // A podcast's script is written by the chat model, also before the card
    // is cleared for a recorded voice.
    let with_script;
    let req = if req.op == "podcast" && req.lyrics.is_none() {
        let script = podcast::write(state, req, cancel, progress).await?;
        with_script = Request { lyrics: Some(script), ..req.clone() };
        &with_script
    } else if req.op == "explainer" && req.lyrics.is_none() {
        let slides = explainer::write(state, req, cancel, progress).await?;
        with_script = Request { lyrics: Some(slides), ..req.clone() };
        &with_script
    } else {
        req
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    // Not for upscaling: its model is 64 MB, and stopping a 14 GB chat model
    // for it cost the next message a reload.
    let forced = ON_PROCESSOR.try_with(|p| *p).unwrap_or(false);
    let paused = if forced {
        None
    } else if music_on_gpu || recorded_voice(req).is_some() || matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "video" | "tune" | "coach" | "cover_art") {
        make_room(state, req, model).await
    } else {
        None
    };
    let result = match model.filter(|m| m.kind != Kind::Upscale) {
        Some(m) if matches!(req.op.as_str(), "generate" | "video") => Box::pin(made_well(state, id, req, m, cancel, progress)).await,
        // Edits are planned and checked unless that's turned off, when something here reads pictures.
        Some(m) if req.op == "edit" && req.check != Some(false) && (crate::vision::can_read(state, None) || installed_file(state, m, "llm_vision").is_some()) => {
            Box::pin(edited_well(state, id, req, m, cancel, progress)).await
        }
        _ => Box::pin(run_kind(state, id, req, model, cancel, progress)).await,
    };
    if let Some(p) = paused {
        give_back(state, p).await;
    }
    result
}

/// A new picture or clip with the prompt writer and the picture reader at
/// work: the description written for this model (with example pictures
/// read), and, when asked, the picture checked against what was asked for
/// and made once more with what was missing, keeping the better one.
async fn made_well(state: &Arc<AppState>, id: &str, req: &Request, m: &MediaModel, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let prepared = match prompts::prepare(state, req, m, progress).await {
        Ok(p) => p,
        Err(e) if e == download::CANCELLED => return Err(e),
        Err(e) => {
            crate::applog::line("media", &format!("The prompt writer couldn't help ({e}); using the description as written"));
            None
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let Some(p) = prepared else { return run_kind(state, id, req, Some(m), cancel, progress).await };
    let mut first_req = req.clone();
    if let Some(w) = &p.written {
        first_req.asked = Some(req.prompt.clone());
        first_req.prompt = w.prompt.clone();
        first_req.negative = (!w.negative.is_empty()).then(|| w.negative.clone());
        // The writer already put the look in.
        first_req.style = Some("as_written".into());
    }
    // Checked: the first make fills part of the bar, and each one after it a share of the rest.
    let checking = p.check && req.op == "generate";
    let slice = |round: usize| -> (f64, f64) {
        let rest = 0.35 / (learn::MAX_MAKES - 1) as f64;
        match round {
            0 if checking => (0.0, 0.6),
            0 => (0.0, 1.0),
            r => (0.6 + rest * (r - 1) as f64, rest),
        }
    };
    let (from, len) = slice(0);
    let bar = |stage: &str, f: f64| progress(stage, from + len * f.clamp(0.0, 1.0));
    let mut items = run_kind(state, id, &first_req, Some(m), cancel, &bar).await?;
    // Checking: one picture, a checklist worth checking, and something to check with.
    if !checking || items.len() != 1 || items[0].kind != "image" || cancel.load(Ordering::Relaxed) {
        return Ok(items);
    }
    let c = state.work_cipher()?;
    // The same seed for every make, so each differs by its words only.
    let seed = items[0].seed;
    let mut item = items.remove(0);
    let mut written = p.written.clone().unwrap_or(prompts::Written { prompt: req.prompt.clone(), negative: req.negative.clone().unwrap_or_default() });
    let mut lesson: Option<String> = None;
    let (mut made, mut rounds, mut tries): (Vec<MediaItem>, Vec<prompts::Round>, Vec<learn::Try>) = (Vec::new(), Vec::new(), Vec::new());
    // What must show, as the reviewer reads the request (once, for every make).
    let mut checklist: Vec<String> = Vec::new();
    loop {
        // The reviewer: what shows, and for what doesn't, why.
        progress("Checking the picture against what you asked for", slice(made.len()).0 + slice(made.len()).1);
        let checked: Result<Option<(prompts::Verdict, Vec<String>)>, String> = async {
            let img = store::read(&state.paths, &c, &item.id).and_then(|b| imaging::decode(&b))?;
            let r = prompts::reader(state, Some(m)).await?;
            if checklist.is_empty() {
                checklist = prompts::checklist(&r, &req.prompt).await;
                crate::applog::line("media", &format!("The reviewer listed {} things to check", checklist.len()));
            }
            // Too little to check against.
            if checklist.len() < 3 {
                r.done(state).await;
                return Ok(None);
            }
            let v = prompts::judge(&r, &img, &checklist).await;
            let notes = match &v {
                Ok(v) if v.score < learn::GOOD => prompts::review(&r, &img, &req.prompt, &written.prompt, &v.missing).await,
                _ => Vec::new(),
            };
            r.done(state).await;
            v.map(|v| Some((v, notes)))
        }
        .await;
        if let Err(e) = &checked {
            crate::applog::line("media", &format!("Couldn't check picture {}: {e}", made.len() + 1));
        }
        let Ok(Some((verdict, notes))) = checked else {
            // Couldn't check it: the first stays as it is; a later one goes.
            if made.is_empty() {
                return Ok(vec![item]);
            }
            let _ = store::delete(&state.db.lock().unwrap(), &state.paths, &item.id);
            break;
        };
        crate::applog::line("media", &format!("Picture {} of at most {}: it showed {:.0}% of what was asked for", made.len() + 1, learn::MAX_MAKES, verdict.score * 100.0));
        tries.push(learn::Try { score: verdict.score, missing: verdict.missing.clone(), lesson: lesson.take(), from: None });
        rounds.push(prompts::Round { written: written.clone(), score: verdict.score, missing: verdict.missing.clone(), notes });
        made.push(item);
        let stop = |why: &str| crate::applog::line("media", &format!("Stopped writing again: {why}"));
        if learn::finished(&tries) || verdict.missing.is_empty() || cancel.load(Ordering::Relaxed) {
            if made.len() > 1 || verdict.score < learn::GOOD {
                stop(if verdict.score >= learn::GOOD || verdict.missing.is_empty() { "it shows what was asked" } else if made.len() >= learn::MAX_MAKES { "the most pictures for one request" } else if cancel.load(Ordering::Relaxed) { "stopped" } else { "two rewrites did no better" });
            }
            break;
        }
        // Model 1 writes again, seeing every try so far and the reviewer's notes.
        let (from, len) = slice(made.len());
        progress("Writing the description again from what was missing", from);
        let Ok(w) = prompts::writer(state, m).await else {
            stop("the writer didn't start");
            break;
        };
        let mut again = prompts::revise(&w, &p.program, &p.ask, &rounds, 0.4).await;
        // The same words again would make the same picture: asked once more, with more variety.
        if again.as_ref().is_ok_and(|(n, _)| rounds.iter().any(|r| r.written.prompt == n.prompt)) {
            again = prompts::revise(&w, &p.program, &p.ask, &rounds, 0.9).await;
        }
        w.done(state).await;
        let (next, said) = match again {
            Ok(x) => x,
            Err(e) => {
                stop(&format!("the writer failed ({e})"));
                break;
            }
        };
        if rounds.iter().any(|r| r.written.prompt == next.prompt) {
            stop("the writer gave back a prompt it had tried, twice");
            break;
        }
        let mut again_req = first_req.clone();
        again_req.asked = Some(req.prompt.clone());
        again_req.prompt = next.prompt.clone();
        again_req.negative = (!next.negative.is_empty()).then(|| next.negative.clone());
        again_req.style = Some("as_written".into());
        again_req.seed = seed;
        again_req.count = Some(1);
        let bar = |stage: &str, f: f64| progress(stage, from + len * f.clamp(0.0, 1.0));
        match run_kind(state, &format!("{id}-{}", made.len()), &again_req, Some(m), cancel, &bar).await {
            Ok(mut s) if s.len() == 1 => item = s.remove(0),
            Ok(s) => {
                for extra in s {
                    let _ = store::delete(&state.db.lock().unwrap(), &state.paths, &extra.id);
                }
                stop("the picture model made more than one");
                break;
            }
            Err(e) => {
                stop(&format!("the picture model failed ({e})"));
                break;
            }
        }
        written = next;
        lesson = said;
    }
    // The best is kept (the first of equals); the others go.
    let best = (0..tries.len()).fold(0, |b, i| if tries[i].score > tries[b].score { i } else { b });
    if made.len() > 1 {
        crate::applog::line("media", &format!("Kept picture {} of {}: {:.0}% of what was asked for (the first {:.0}%)", best + 1, made.len(), tries[best].score * 100.0, tries[0].score * 100.0));
    }
    let mut keep = Vec::new();
    for (i, it) in made.into_iter().enumerate() {
        if i == best {
            keep.push(it);
        } else {
            let _ = store::delete(&state.db.lock().unwrap(), &state.paths, &it.id);
        }
    }
    if learn::may_learn(state, req.chat_id.as_deref()) {
        learn_from_picture(state, m, req, &p, &rounds, &tries, best);
    }
    Ok(if tries[best].score < learn::GOOD { said_missing(state, keep, &tries[best].missing) } else { keep })
}

/// What the writer keeps from a checked picture: how its first try did, the
/// rules that made missing things show, and the prompt that passed.
fn learn_from_picture(state: &AppState, m: &MediaModel, req: &Request, p: &prompts::Prepared, rounds: &[prompts::Round], tries: &[learn::Try], best: usize) {
    let mut l = learn::load(state, m);
    let now = db::now_ms();
    learn::add_first(&mut l.firsts, tries[0].score);
    let rules = learn::confirmed(tries);
    for r in &rules {
        learn::add_rule(&mut l.rules, r, now);
    }
    // A prompt the writer wrote (not the person's words as given) that passed.
    let wrote = p.written.is_some() || best > 0;
    let passed = tries[best].score >= learn::GOOD && wrote;
    if passed {
        let w = &rounds[best].written;
        let answer = json!({ "prompt": w.prompt, "negative": w.negative }).to_string();
        learn::add_example(&mut l.pictures, learn::Example { request: req.prompt.clone(), ask: p.ask.render(), answer, score: tries[best].score, makes: best as u32 + 1, at: now });
    }
    match learn::save(state, m, &l) {
        Ok(()) => crate::applog::line("media", &format!("The prompt writer learnt from this picture: {}{} ({} examples and {} rules for {} so far)", if passed { "an example" } else { "its first try's score" }, if rules.is_empty() { String::new() } else { format!(", {} rule(s)", rules.len()) }, l.pictures.len(), l.rules.len(), m.name)),
        Err(e) => crate::applog::line("media", &format!("Couldn't keep what the prompt writer learnt: {e}")),
    }
}

/// An edit with the picture reader at work. Asked to have a child lift the
/// barbell in front of her overhead, the edit model drew a second barbell
/// above her and left the first on the floor, with plates missing or of
/// another kind, and the result came back smaller than the photo. So:
/// - the reader looks at the picture and plans the request as steps the
///   edit model makes well (a move as "take it away", then "put it there,
///   looking like this"), with a checklist that includes what must be gone;
/// - the steps run one after another, each on the last one's result;
/// - the reader checks the result against the picture before, and when it
///   falls short the steps are planned again from what was wrong and made
///   once more, keeping the better of the two;
/// - the result goes back into the full-size original, so whatever the edit
///   didn't change keeps its own pixels.
async fn edited_well(state: &Arc<AppState>, id: &str, req: &Request, m: &MediaModel, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let reserved = chat_model_vram(state).await;
    let ctx = RunCtx { state, req, cancel, progress, reserved };
    let work = work_dir(&state.paths, id)?;
    let (item, full) = ctx.source()?;
    let d = &m.defaults;
    let hw = state.hardware.read().unwrap().clone();
    let on_gpu = fit(m, &hw, &state.budget(), 0, None).on_gpu;
    let side = if on_gpu { d.width.max(d.height).max(512) } else { 640 };
    let (w, h) = imaging::work_size(full.width(), full.height(), side * side);
    let (w, h) = if d.align > 16 { ((w / d.align).max(1) * d.align, (h / d.align).max(1) * d.align) } else { (w, h) };
    let before = full.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    // The first picture of an edit chain keeps faces from drifting (see picture()).
    let original = if changes_how_people_look(&req.prompt) { None } else { ctx.original_of(&item)? };
    let stopped = || cancel.load(Ordering::Relaxed);

    progress("Looking at the picture and planning the change", 0.0);
    // What the planner has learnt about this edit model: rules, and edits like this one.
    let help = learn::planner_help(&learn::load(state, m), &req.prompt);
    let coached = coach::trained(state, m).is_some();
    // One picture reader for the whole edit when the card holds it beside
    // the picture model: it was started and stopped three to five times.
    let mut kept: Option<crate::vision::Helper> = None;
    let (plan, rubric) = match prompts::reader(state, Some(m)).await {
        Ok(r) => {
            let p = prompts::plan_edit_with(&r, &before, &req.prompt, &help).await;
            // A writer coached for this model plans from a description of
            // the picture, the way it practised.
            let caption = if coached { prompts::caption(&r, &before).await.ok() } else { None };
            // (Not kept when the coached writer plans next: one helper at a time.)
            if !coached && reader_fits(state, &r, m).await {
                kept = Some(r);
            } else {
                r.done(state).await;
            }
            let p = p.map_err(|e| crate::applog::line("media", &format!("Couldn't plan the edit ({e}); using the instruction as written"))).ok();
            let mine = match caption {
                Some(c) => match prompts::writer(state, m).await {
                    Ok(w) => {
                        let x = prompts::plan_from_words(&w, &c, &req.prompt, 0.3).await.ok();
                        w.done(state).await;
                        x
                    }
                    Err(_) => None,
                },
                None => None,
            };
            if mine.is_some() {
                crate::applog::line("media", "The coached prompt writer planned the edit");
            }
            // The reader's plan says what to check, either way.
            (mine.or_else(|| p.clone()), p)
        }
        Err(e) => {
            crate::applog::line("media", &format!("Nothing could look at the picture ({e}); using the instruction as written"));
            (None, None)
        }
    };
    let plan = plan.unwrap_or_else(|| prompts::EditPlan { steps: vec![req.prompt.trim().to_string()], ..Default::default() });
    let check = rubric.unwrap_or_else(|| plan.clone());
    crate::applog::line("media", &format!("Edit planned as {} step(s) with {} things to check", plan.steps.len(), plan.checklist.len()));
    if stopped() {
        return Err(download::CANCELLED.into());
    }
    let seed = req.seed.unwrap_or_else(random_seed);
    // Something taken away or moved: a change in one place, so the rest stays put.
    let local = plan.changes["leaving"].as_array().is_some_and(|l| !l.is_empty());
    // The picture model leaves room for a reader kept on the card.
    let room = |k: &Option<crate::vision::Helper>| RunCtx { state, req, cancel, progress, reserved: reserved + k.as_ref().map_or(0, |r| r.card_bytes) };
    // Made, checked by the reader and planned again from what was missing,
    // up to `learn::MAX_MAKES` times; the best is kept.
    let checks = check.checklist.len() >= 2;
    let (mut plan_now, mut seed_now, mut lesson) = (plan.clone(), seed, None::<String>);
    let mut tries: Vec<learn::Try> = Vec::new();
    // The best so far: its points, the picture, whether it redrew the whole
    // scene, its verdict, its plan and which make it was.
    let mut best: Option<(f32, image::DynamicImage, bool, Option<prompts::Verdict>, prompts::EditPlan, usize)> = None;
    // The best one as the edit model made it, and which try it was.
    let mut best_made: Option<(image::DynamicImage, Option<usize>)> = None;
    // The try the next plan was made from.
    let mut from: Option<usize> = None;
    let rest = 0.45 / (learn::MAX_MAKES - 1) as f64;
    for round in 0..learn::MAX_MAKES {
        let slice = if round == 0 { (0.1, 0.45) } else { (0.55 + rest * (round - 1) as f64, rest * 0.7) };
        if round > 0 {
            progress("Making it again so it shows everything you asked for", slice.0);
        }
        let made = match edit_steps(&room(&kept), m, &before, original.as_ref(), &plan_now.steps, seed_now, &work.0.join(round.to_string()), slice).await {
            Ok(x) => x,
            Err(e) if round == 0 => return Err(e),
            Err(_) => break,
        };
        // Back into the full-size original, when it fits (None: the edit model redrew the whole scene).
        let made_kept = imaging::keep_unchanged(&full, &before, &made);
        let whole = local && made_kept.is_none();
        if whole {
            crate::applog::line("media", "The edit redrew the whole scene, so it doesn't fit back into the original");
        }
        let (mut verdict, mut next, mut reader) = (None, None, None);
        if checks && !stopped() {
            progress("Checking the change against what you asked for", slice.0 + slice.1);
            let r = match kept.take() {
                Some(r) => Ok(r),
                None => prompts::reader(state, Some(m)).await,
            };
            if let Ok(r) = r {
                if let Ok(v) = prompts::judge_edit(&r, &before, &made, &check).await {
                    crate::applog::line("media", &format!("Edit {} of at most {}: it did {:.0}% of what was asked for", round + 1, learn::MAX_MAKES, v.score * 100.0));
                    tries.push(learn::Try { score: v.score, missing: v.missing.clone(), lesson: lesson.take(), from: from.take() });
                    verdict = Some(v);
                }
                reader = Some(r);
            }
        }
        // The better one, with a little extra for one that fits back into the original.
        let points = verdict.as_ref().map_or(0.0, |v| v.score) + if local && made_kept.is_some() { 0.1 } else { 0.0 };
        if round > 0 {
            crate::applog::line("media", &format!("Made again: {:.0} points against {:.0} for the best before", points * 100.0, best.as_ref().map_or(0.0, |b| b.0) * 100.0));
        }
        if best.as_ref().is_none_or(|b| points > b.0) {
            best = Some((points, made_kept.clone().unwrap_or_else(|| made.clone()), made_kept.is_none(), verdict.clone(), plan_now.clone(), round));
            best_made = Some((made.clone(), tries.len().checked_sub(1)));
        }
        // Planned again from the best so far (its picture and what it
        // missed), not the last: after a worse try, that wandered off.
        if let Some(r) = reader {
            if let (Some((_, _, _, Some(bv), bplan, _)), Some((bmade, bi))) = (best.as_ref(), best_made.as_ref()) {
                if bv.score < learn::GOOD && !bv.missing.is_empty() && !learn::finished(&tries) && !stopped() {
                    next = prompts::replan_edit(&r, &before, bmade, &req.prompt, bplan, &bv.missing, &help).await.ok();
                    from = *bi;
                }
            }
            if reader_fits(state, &r, m).await {
                kept = Some(r);
            } else {
                r.done(state).await;
            }
        }
        if stopped() {
            break;
        }
        // Next: a new plan for what was missing, with the same seed so it
        // differs by its plan; or, when the first redrew the whole scene for
        // a change in one place, the same steps with a new seed.
        match next {
            Some((p, said)) => {
                plan_now = p;
                lesson = said;
                if whole {
                    seed_now = random_seed();
                }
            }
            None if round == 0 && whole => seed_now = random_seed(),
            None => break,
        }
    }
    let (_, mut result, redrawn, v1, best_plan, best_round) = best.ok_or("The edit didn't come out.")?;
    if learn::may_learn(state, req.chat_id.as_deref()) && !tries.is_empty() {
        learn_from_edit(state, m, req, &tries, v1.as_ref(), &best_plan, best_round, kept.as_ref(), &before).await;
    }
    if let Some(r) = kept.take() {
        r.done(state).await;
    }
    if stopped() {
        return Err(download::CANCELLED.into());
    }
    // The whole scene redrawn: back to the photo's size (with the upscaler
    // when there is one), not left at the edit model's working size.
    let mut note = None;
    if redrawn && result.dimensions() != full.dimensions() {
        let big = ctx.enlarge_for(&result, full.width(), full.height(), &work.0).await;
        result = big.resize_exact(full.width(), full.height(), image::imageops::FilterType::Lanczos3);
        note = Some("The edit redrew the whole scene, so everything in it was drawn again (at the edit model's detail) rather than kept from the photo.".to_string());
    }
    let kept = ctx.keep_picture(&result, Some(m), Some(&item.id), Some(seed))?;
    let missing = v1.filter(|v| v.score < 0.85).map(|v| v.missing).unwrap_or_default();
    let mut items = said_missing(state, vec![kept], &missing);
    if let Some(n) = note {
        if let Some(i) = items.first_mut() {
            i.note = Some(n);
            if let Ok(c) = state.work_cipher() {
                let _ = store::update(&state.db.lock().unwrap(), &c, i);
            }
        }
    }
    Ok(items)
}

/// What the planner keeps from a checked edit: how its first plan did, the
/// rules that made missing things show, and the plan that passed (with the
/// picture in words: the reader's, when it's still on the card).
#[allow(clippy::too_many_arguments)]
async fn learn_from_edit(state: &AppState, m: &MediaModel, req: &Request, tries: &[learn::Try], best: Option<&prompts::Verdict>, plan: &prompts::EditPlan, round: usize, reader: Option<&crate::vision::Helper>, before: &image::DynamicImage) {
    let mut l = learn::load(state, m);
    let now = db::now_ms();
    learn::add_first(&mut l.edit_firsts, tries[0].score);
    let rules = learn::confirmed(tries);
    for r in &rules {
        learn::add_rule(&mut l.edit_rules, r, now);
    }
    let passed = best.is_some_and(|v| v.score >= learn::GOOD) && !plan.changes.is_null();
    if passed {
        let words = match reader {
            Some(r) => prompts::caption(r, before).await.ok(),
            None => None,
        };
        let scene = words.unwrap_or_else(|| learn::scene_of(&plan.changes));
        let ask = format!("The request: {} (The picture: {})", req.prompt.trim().trim_end_matches('.'), scene);
        learn::add_example(&mut l.edits, learn::Example { request: req.prompt.clone(), ask, answer: plan.changes.to_string(), score: best.map_or(0.0, |v| v.score), makes: round as u32 + 1, at: now });
    }
    match learn::save(state, m, &l) {
        Ok(()) => crate::applog::line("media", &format!("The edit planner learnt from this edit: {}{} ({} examples and {} rules for {} so far)", if passed { "an example" } else { "its first plan's score" }, if rules.is_empty() { String::new() } else { format!(", {} rule(s)", rules.len()) }, l.edits.len(), l.edit_rules.len(), m.name)),
        Err(e) => crate::applog::line("media", &format!("Couldn't keep what the edit planner learnt: {e}")),
    }
}

/// A picture reader on its own engine can stay on the card through an
/// edit when the card holds it, the chat model's share and the picture
/// model (with a gigabyte to spare).
async fn reader_fits(state: &Arc<AppState>, r: &crate::vision::Helper, m: &MediaModel) -> bool {
    let vram = state.budget().vram_card;
    let chat = chat_model_vram(state).await;
    let picture = (m.min_vram_gb * GIB as f64) as u64;
    r.card_bytes > 0 && vram >= chat + r.card_bytes + picture + GIB
}

/// The kept picture says what the check didn't find in it, so the person
/// hears it (the Studio card, the chat) instead of it only reaching the log.
fn said_missing(state: &AppState, mut items: Vec<MediaItem>, missing: &[String]) -> Vec<MediaItem> {
    if missing.is_empty() || items.len() != 1 {
        return items;
    }
    items[0].missing = missing.to_vec();
    if let Ok(c) = state.work_cipher() {
        let _ = store::update(&state.db.lock().unwrap(), &c, &items[0]);
    }
    items
}

/// Runs an edit's steps in order, each on the last one's result, at the
/// size of `before`. Nothing is kept in the gallery.
/// `slice`: the part of the job's bar (start, length) these steps fill.
async fn edit_steps(ctx: &RunCtx<'_>, m: &MediaModel, before: &image::DynamicImage, original: Option<&image::DynamicImage>, steps: &[String], seed: i64, dir: &std::path::Path, slice: (f64, f64)) -> Result<image::DynamicImage, String> {
    let d = &m.defaults;
    let (w, h) = (before.width(), before.height());
    let mut img = before.clone();
    let n = steps.len().max(1) as f64;
    for (i, step) in steps.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        let part = |label: &str, f: f64| (ctx.progress)(label, slice.0 + slice.1 * (i as f64 + f.clamp(0.0, 1.0)) / n);
        let ctx = &RunCtx { state: ctx.state, req: ctx.req, cancel: ctx.cancel, progress: &part, reserved: ctx.reserved };
        let dir = dir.join(i.to_string());
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let src = dir.join("source.png");
        std::fs::write(&src, imaging::png(&img)?).map_err(|e| e.to_string())?;
        let mut refs = vec![src];
        let mut prompt = step.trim().to_string();
        if let Some(o) = original {
            let first = dir.join("original.png");
            std::fs::write(&first, imaging::png(&o.resize(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
            refs.push(first);
            prompt = format!(
                "{}. Work on the first picture and keep everything in it that isn't mentioned. The second picture is the original: keep every person's face and features exactly as there.",
                prompt.trim_end_matches('.')
            );
        }
        let negative = (!d.negative.is_empty()).then(|| fit_negative(&d.negative, None, &prompt)).filter(|n| !n.is_empty());
        let out = dir.join("out_%d.png");
        let args = sd::picture_args(
            &ctx.files(m),
            d,
            &sd::Picture {
                prompt: &prompt,
                negative: negative.as_deref(),
                width: w,
                height: h,
                steps: ctx.req.steps.unwrap_or(d.steps).clamp(1, 60),
                seed: seed + i as i64,
                count: 1,
                init: None,
                mask: None,
                refs: refs.iter().map(|p| p.as_path()).collect(),
                strength: None,
                out: &out,
            },
            &ctx.limits(),
        );
        ctx.run_sd(args, &dir).await?;
        let made = std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("out_")));
        let next = imaging::decode(&std::fs::read(made.ok_or("The picture engine finished without making a picture.")?).map_err(|e| e.to_string())?)?;
        // A run that fell back to a smaller size: back to this size for the next step.
        img = if next.dimensions() == (w, h) { next } else { next.resize_exact(w, h, image::imageops::FilterType::Lanczos3) };
        // Each step's picture, to look at while testing (SULCUSAI_EDIT_STEPS: a folder).
        if let Some(out) = std::env::var_os("SULCUSAI_EDIT_STEPS") {
            let attempt = dir.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = std::fs::write(std::path::Path::new(&out).join(format!("{attempt}-{}.png", i + 1)), imaging::png(&img).unwrap_or_default());
        }
    }
    Ok(img)
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

/// Picture jobs: make, edit, fill in, extend, restyle.
async fn picture(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let d = &m.defaults;
    let hw = ctx.state.hardware.read().unwrap().clone();
    let on_gpu = fit(m, &hw, &ctx.state.budget(), 0, None).on_gpu;
    // On the processor, smaller pictures keep the wait reasonable.
    let side = if on_gpu { d.width.max(d.height).max(512) } else { 640 };
    let seed = req.seed.unwrap_or_else(random_seed);
    let steps = req.steps.unwrap_or(d.steps).clamp(1, 60);
    let out = work.join("out_%d.png");
    // Some models want sides in steps of 32.
    let al = |(w, h): (u32, u32)| if d.align > 16 { ((w / d.align).max(1) * d.align, (h / d.align).max(1) * d.align) } else { (w, h) };
    let mut source: Option<(MediaItem, image::DynamicImage)> = None;
    let mut fill_mask: Option<image::GrayImage> = None;
    // An edit: the picture as the model got it, to find what it changed.
    let mut edit_before: Option<image::DynamicImage> = None;
    let mut count = 1;
    let (prompt, w, h, init, mask, refs) = match req.op.as_str() {
        "generate" => {
            count = if req.wallpaper.is_some() { 1 } else { req.count.unwrap_or(1).clamp(1, 4) };
            let (w, h) = al(match (&req.wallpaper, req.width, req.height) {
                (Some(spec), _, _) => wallpaper::make_size(spec, side, d.align),
                (None, Some(w), Some(h)) => (imaging::round16(w.clamp(256, 2048) as f64), imaging::round16(h.clamp(256, 2048) as f64)),
                _ => imaging::shape_size(req.shape.as_deref().unwrap_or("square"), side),
            });
            // Example pictures: references for models that read them (image 1,
            // image 2…); for the layout (or with nothing to read them), the
            // picture starts from the first one and is redrawn partway.
            let examples = ctx.examples()?;
            let (mut init, mut refs) = (None, vec![]);
            // For a style or a subject, a model that can't take them gets the
            // picture reader's words only: starting from the example copied its
            // room, layout and even its sign into a picture asked to share only
            // its look. Without a reader, the layout is all that can be used.
            let layout = req.example_use.as_deref() == Some("layout");
            let words_only = !layout && (crate::vision::can_read(ctx.state, None) || installed_file(ctx.state, m, "llm_vision").is_some());
            if !examples.is_empty() && !(words_only && !m.can("edit")) {
                if m.can("edit") && !layout {
                    for (i, img) in examples.iter().enumerate() {
                        let (ew, eh) = imaging::work_size(img.width(), img.height(), w * h);
                        let p = work.join(format!("example{i}.png"));
                        std::fs::write(&p, imaging::png(&img.resize_exact(ew, eh, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
                        refs.push(p);
                    }
                } else {
                    let p = work.join("start.png");
                    std::fs::write(&p, imaging::png(&imaging::fill_to(&examples[0], w, h))?).map_err(|e| e.to_string())?;
                    init = Some(p);
                }
            }
            // A pose from a picture: given to a model that reads pictures as
            // the last image, with only its pose to be taken.
            let mut pose_at = None;
            if let Some(pid) = req.pose.as_deref().filter(|p| !p.is_empty()) {
                if !m.can("edit") {
                    return Err("Taking a pose from a picture needs a picture model that reads pictures: FLUX.2 [klein] or Qwen-Image (Studio › Models).".into());
                }
                let c = ctx.state.work_cipher()?;
                let img = store::read(&ctx.state.paths, &c, pid).and_then(|b| imaging::decode(&b)).map_err(|_| "The pose picture is no longer in the gallery.".to_string())?;
                // Its pose only: given as it was, the model copied the face too.
                let img = pose_only(ctx, img).await;
                let (pw, ph) = imaging::work_size(img.width(), img.height(), w * h);
                let p = work.join("pose.png");
                std::fs::write(&p, imaging::png(&img.resize_exact(pw, ph, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
                init = None;
                refs.push(p);
                pose_at = Some(refs.len());
            }
            // With their own LoRAs and the look left to Auto, the LoRA sets the
            // look: Auto's photo words turned a pixel-art LoRA back into a photo.
            let look = if !req.loras.is_empty() && matches!(req.style.as_deref(), None | Some("auto")) { Some("as_written") } else { req.style.as_deref() };
            let mut prompt = styled(&req.prompt, look);
            // The same character as the example: said plainly to a model that
            // gets it as a reference (the prompt writer says it its own way).
            let examples_given = pose_at.map_or(refs.len(), |k| k - 1);
            if req.example_use.as_deref() == Some("subject") && examples_given > 0 && req.write_prompt != Some(true) {
                let which = if examples_given == 1 { "image 1".to_string() } else { format!("images 1 to {examples_given}") };
                prompt = format!("{}. The same character as in {which}: keep the face, hair, body, clothes and colors exactly the same.", prompt.trim_end_matches('.'));
            }
            if let Some(k) = pose_at {
                prompt = format!("{}. The body pose exactly as the figure in image {k}: the same position of the head, arms and legs. Image {k} only shows the pose.", prompt.trim_end_matches('.'));
            }
            if let Some(spec) = req.wallpaper.as_ref().filter(|_| req.style.as_deref() != Some("as_written")) {
                prompt.push_str(wallpaper::plain_words(spec));
            }
            (prompt, w, h, init, None, refs)
        }
        "edit" | "restyle" => {
            let s = ctx.source()?;
            let (w, h) = al(imaging::work_size(s.1.width(), s.1.height(), side * side));
            let src = work.join("source.png");
            let small = s.1.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
            std::fs::write(&src, imaging::png(&small)?).map_err(|e| e.to_string())?;
            if req.op == "edit" {
                edit_before = Some(small);
            }
            let mut prompt = if req.op == "restyle" {
                format!("Redraw this picture in this style: {}. Keep the same subject, layout and composition.", req.prompt.trim())
            } else {
                req.prompt.trim().to_string()
            };
            let mut refs = vec![src];
            // An edit of an edit: the first picture goes along too, so faces and
            // details don't drift a little further with every edit in a chain
            // (a few edits in, pictures had lost what was asked for).
            // Not when the change is to how people look (older, a new hairdo,
            // a cartoon of them): the original's faces would undo it.
            let changes_looks = req.op == "restyle" || changes_how_people_look(&req.prompt);
            if let Some(original) = if changes_looks { None } else { ctx.original_of(&s.0)? } {
                let first = work.join("original.png");
                std::fs::write(&first, imaging::png(&original.resize(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
                refs.push(first);
                prompt = format!(
                    "{}. Work on the first picture and keep everything in it that isn't mentioned. The second picture is the original: keep every person's face and features exactly as there.",
                    prompt.trim_end_matches('.')
                );
            }
            source = Some(s);
            (prompt, w, h, None, None, refs)
        }
        "fill" => {
            let s = ctx.source()?;
            let (w, h) = al(imaging::work_size(s.1.width(), s.1.height(), side * side));
            let bytes = base64::engine::general_purpose::STANDARD.decode(req.mask.as_deref().unwrap_or("")).map_err(|_| "The painted area couldn't be read.".to_string())?;
            let mk = imaging::grow(&imaging::mask_from_paint(&bytes, w, h)?, 4);
            let (src, mpath) = (work.join("source.png"), work.join("mask.png"));
            std::fs::write(&src, imaging::png(&s.1.resize_exact(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
            std::fs::write(&mpath, imaging::png(&image::DynamicImage::ImageLuma8(mk.clone()))?).map_err(|e| e.to_string())?;
            let prompt = if req.prompt.trim().is_empty() { "Fill in this area naturally so it matches its surroundings.".to_string() } else { req.prompt.trim().to_string() };
            fill_mask = Some(mk);
            source = Some(s);
            (prompt, w, h, Some(src), Some(mpath), vec![])
        }
        "extend" => {
            let s = ctx.source()?;
            let [l, t, r, b] = req.extend.unwrap_or_else(|| {
                // Default: wider, a third on each side.
                let add = s.1.width() / 3;
                [add, 0, add, 0]
            });
            if l + t + r + b == 0 {
                return Err("Choose which sides to extend.".into());
            }
            let (full, full_mask) = imaging::extend(&s.1, l, t, r, b);
            let (w, h) = al(imaging::work_size(full.width(), full.height(), side * side));
            let full = image::DynamicImage::ImageRgba8(full);
            let canvas = full.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
            let mk = image::imageops::resize(&full_mask, w, h, image::imageops::FilterType::Nearest);
            let (src, mpath) = (work.join("source.png"), work.join("mask.png"));
            std::fs::write(&src, imaging::png(&canvas)?).map_err(|e| e.to_string())?;
            std::fs::write(&mpath, imaging::png(&image::DynamicImage::ImageLuma8(mk))?).map_err(|e| e.to_string())?;
            let prompt = if req.prompt.trim().is_empty() {
                "Continue the picture naturally beyond its edges, with the same style, lighting and detail.".to_string()
            } else {
                req.prompt.trim().to_string()
            };
            // The new edges go around the original at full size (it came
            // back smaller than the picture it started from).
            fill_mask = Some(full_mask);
            source = Some((s.0, full));
            (prompt, w, h, Some(src), Some(mpath), vec![])
        }
        _ => return Err("Unknown kind of picture job.".into()),
    };
    let negative = req
        .negative
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .or((!d.negative.is_empty()).then_some(d.negative.as_str()))
        .map(|n| fit_negative(n, req.style.as_deref(), &prompt))
        // A wallpaper is the scene alone, never a picture of a desktop.
        .map(|n| if req.wallpaper.is_some() { format!("{n}, user interface, taskbar, desktop icons, window frame, text") } else { n })
        // With their own LoRAs, the model's usual "no drawings" words go: they
        // fought a pixel-art LoRA into a photo.
        .map(|n| if req.loras.is_empty() { n } else { without_style_words(&n) })
        .filter(|n| !n.is_empty());
    // Their own LoRAs: named in the prompt, loaded from their folder.
    let loras = loras::for_job(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &m.id, &req.loras);
    let prompt = match &loras {
        Some((words, _)) => format!("{prompt}, {words}"),
        None => prompt,
    };
    let mut args = sd::picture_args(
        &ctx.files(m),
        d,
        &sd::Picture {
            prompt: &prompt,
            negative: negative.as_deref(),
            width: w,
            height: h,
            steps,
            seed,
            count,
            init: init.as_deref(),
            mask: mask.as_deref(),
            refs: refs.iter().map(|p| p.as_path()).collect(),
            // Filling in redraws the masked area whole; starting from an example keeps its layout and colors.
            strength: match (&init, &mask) {
                (Some(_), Some(_)) => Some(1.0),
                (Some(_), None) => Some(0.62),
                _ => None,
            },
            out: &out,
        },
        &ctx.limits(),
    );
    if let Some((_, folder)) = &loras {
        args.extend(["--lora-model-dir".into(), folder.clone().into_os_string()]);
    }
    ctx.run_sd(args, work).await?;
    let mut made: Vec<PathBuf> = std::fs::read_dir(work)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("out_")))
        .collect();
    made.sort();
    if made.is_empty() {
        return Err("The picture engine finished without making a picture.".into());
    }
    let mut items = Vec::new();
    for (i, p) in made.iter().enumerate() {
        let mut img = imaging::decode(&std::fs::read(p).map_err(|e| e.to_string())?)?;
        // Filling in: put the new area into the full-size original.
        if let (Some(mk), Some((_, orig))) = (&fill_mask, &source) {
            img = imaging::composite(orig, &img, mk);
        } else if let (Some(b), Some((_, orig))) = (&edit_before, &source) {
            // An edit: what it didn't change stays the original's, at its size.
            if let Some(kept) = imaging::keep_unchanged(orig, b, &img) {
                img = kept;
            }
        }
        if let Some(spec) = req.wallpaper.as_ref().filter(|_| req.op == "generate") {
            let big = ctx.enlarge_for(&img, spec.width, spec.height, work).await;
            let done = wallpaper::finish(&big, spec);
            if spec.oled {
                crate::applog::line("media", &format!("OLED wallpaper: {:.0}% of its pixels are off", wallpaper::black_share(&done.to_rgba8()) * 100.0));
            }
            items.push(ctx.keep_as("wallpaper", &done, Some(m), None, Some(seed))?);
            continue;
        }
        items.push(ctx.keep_picture(&img, Some(m), source.as_ref().map(|s| s.0.id.as_str()), Some(seed + i as i64))?);
    }
    Ok(items)
}

async fn upscale(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let (item, img) = ctx.source()?;
    // Four times a picture up to 2048 pixels, kept within 4096 (shrinking a
    // big photo to 1024 first gave back no more detail than it had).
    let long = img.width().max(img.height());
    if long > 2048 {
        return Err(format!("This picture is already large ({} × {}). Upscaling is for pictures up to 2048 pixels across.", img.width(), img.height()));
    }
    let (src, out) = (work.join("source.png"), work.join("out.png"));
    std::fs::write(&src, imaging::png(&img)?).map_err(|e| e.to_string())?;
    let args = sd::upscale_args(&ctx.files(m), &src, &out, &ctx.limits());
    ctx.run_sd(args, work).await?;
    let big = imaging::cap(imaging::decode(&std::fs::read(&out).map_err(|_| "The upscaler didn't produce a picture.".to_string())?)?, 4096);
    // A cut-out stays one: its transparency, enlarged to match.
    let big = imaging::keep_alpha(&img, big);
    Ok(vec![ctx.keep_picture(&big, Some(m), Some(&item.id), None)?])
}

async fn remove_background(ctx: &RunCtx<'_>, m: &MediaModel) -> Result<Vec<MediaItem>, String> {
    let (item, img) = ctx.source()?;
    let dll = crate::natural::runtime_dll(&ctx.state.paths, &ctx.state.catalog.voices).ok_or("ONNX Runtime isn't installed. Reinstall BiRefNet from the Studio's Models list.")?;
    let f = m.file("onnx").ok_or("The background model is missing a file.")?;
    let model_path = file_path(&ctx.state.paths, m, f);
    let threads = ctx.state.limits().threads.max(2);
    (ctx.progress)("Finding the subject", 0.2);
    let (img2, out) = tokio::task::spawn_blocking(move || -> Result<(image::DynamicImage, image::RgbaImage), String> {
        let matte = cutout::matte(&dll, &model_path, &img, threads)?;
        let out = cutout::apply(&img, &matte);
        Ok((img, out))
    })
    .await
    .map_err(|e| e.to_string())??;
    drop(img2);
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    Ok(vec![ctx.keep_picture(&image::DynamicImage::ImageRgba8(out), Some(m), Some(&item.id), None)?])
}

async fn video(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let d = &m.defaults;
    let fps = d.fps.max(8);
    // No longer than the model was trained on (Wan 2.2: 121 frames, Wan 2.1: 81).
    let max_frames = if d.max_frames > 0 { d.max_frames } else { 81 };
    let longest = (max_frames as f32 - 1.0) / fps as f32;
    // Longer: in parts, each going on from the last frame of the one before
    // (models that start from a picture; the parts are joined on Windows).
    // A clip with its own sound is made in one go (parts would cut the sound up).
    let sound = m.file("audio_vae").is_some();
    let can_continue = m.can("image") && cfg!(windows) && req.wallpaper.is_none() && !sound;
    let most = if can_continue { longclip::MAX_SECS } else { longest };
    let seconds = req.seconds.unwrap_or(if req.wallpaper.is_some() { longest } else { d.seconds }).clamp(1.0, most.max(1.0));
    let total = (seconds * fps as f32).round() as u32 + 1;
    let parts = match () {
        _ if can_continue => longclip::plan(total, max_frames),
        // LTX: 8n+1 frames.
        _ if sound => vec![sd::frames_in_steps(seconds, fps, 8).min(max_frames)],
        _ => vec![sd::frames_for(seconds, fps).min(max_frames)],
    };
    let frames = parts[0];
    let (mut w, mut h) = (d.width.max(256), d.height.max(256));
    if let Some(spec) = &req.wallpaper {
        (w, h) = wallpaper::make_size(spec, imaging::round16(((w * h) as f64).sqrt()), 32);
    } else if let (Some(rw), Some(rh)) = (req.width, req.height) {
        // A size asked for, in the steps of 32 the video models work in.
        let step = |v: u32| (v.clamp(256, 1920) + 16) / 32 * 32;
        (w, h) = (step(rw), step(rh));
    } else if matches!(req.shape.as_deref(), Some("portrait" | "tall")) {
        std::mem::swap(&mut w, &mut h);
    } else if req.shape.as_deref() == Some("square") {
        let s = imaging::round16(((w * h) as f64).sqrt());
        (w, h) = (s, s);
    }
    let mut parent = None;
    // A clip made from a picture starts on it: that's its thumbnail.
    let mut still = None;
    let init = match req.source.as_deref() {
        Some(_) => {
            let (item, img) = ctx.source()?;
            // Same shape as the picture, at the model's size.
            let (iw, ih) = img.dimensions();
            let area = (w * h) as f64;
            let k = (area / (iw as f64 * ih as f64)).sqrt();
            // Within what the models take (a wide panorama asked for 2832x144 and failed after loading).
            (w, h) = (imaging::round16(iw as f64 * k).clamp(256, 1920), imaging::round16(ih as f64 * k).clamp(256, 1920));
            let p = work.join("start.png");
            let start = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
            std::fs::write(&p, imaging::png(&start)?).map_err(|e| e.to_string())?;
            still = Some(imaging::thumbnail(&start)?);
            parent = Some(item.id);
            Some(p)
        }
        None => None,
    };
    let mut prompt = if req.prompt.trim().is_empty() { "Gentle natural motion, the scene comes to life.".to_string() } else { req.prompt.trim().to_string() };
    if let Some(spec) = req.wallpaper.as_ref().filter(|_| req.style.as_deref() != Some("as_written")) {
        prompt.push_str(wallpaper::plain_words(spec));
        prompt.push_str(", slow continuous gentle motion, camera nearly still");
    }
    let seed = req.seed.unwrap_or_else(random_seed);
    if parts.len() > 1 {
        return long_video(ctx, m, work, LongClip { prompt: &prompt, parts: &parts, total, fps, w, h, seed, init, parent, still }).await;
    }
    // A wallpaper's frames come out as pictures, to be fitted to the screen and looped.
    let out = if req.wallpaper.is_some() && cfg!(windows) { work.join("frame_%03d.png") } else { work.join("out.webm") };
    let args = sd::clip_args(
        &ctx.files(m),
        d,
        &sd::Clip {
            prompt: &prompt,
            negative: req.negative.as_deref(),
            width: w,
            height: h,
            frames,
            fps,
            steps: req.steps.unwrap_or(d.steps).clamp(4, 50),
            seed,
            init: init.as_deref(),
            audio: None,
            out: &out,
        },
        &ctx.limits(),
    );
    ctx.run_sd(args, work).await?;
    let c = ctx.state.work_cipher()?;
    if let Some(spec) = req.wallpaper.as_ref().filter(|_| cfg!(windows)) {
        let files = wallpaper::frame_files(work);
        (ctx.progress)("Fitting it to your screen and making it loop", 0.0);
        let mp4 = work.join("wallpaper.mp4");
        let (bytes, poster) = tokio::task::block_in_place(|| wallpaper::encode_loop(&files, spec, fps, &mp4, ctx.cancel, &|f| (ctx.progress)("Fitting it to your screen and making it loop", f)))?;
        let (cw, ch) = wallpaper::clip_size(spec);
        let shown = wallpaper::loop_plan(files.len()).len();
        let item = MediaItem {
            kind: "video".into(),
            op: "wallpaper".into(),
            prompt: req.asked.as_deref().unwrap_or(&req.prompt).trim().to_string(),
            model_id: Some(m.id.clone()),
            mime: "video/mp4".into(),
            width: cw,
            height: ch,
            seconds: shown as f64 / fps as f64,
            chat_id: req.chat_id.clone(),
            seed: Some(seed),
            poster: true,
            ..Default::default()
        };
        let thumb = imaging::thumbnail(&poster)?;
        return Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &bytes, Some(&thumb))?]);
    }
    let (out, mime) = if sound { with_its_sound(ctx, &out, work).await? } else { (out, "video/webm") };
    let bytes = std::fs::read(&out).map_err(|_| "The video engine didn't produce a clip.".to_string())?;
    let item = MediaItem {
        kind: "video".into(),
        op: "video".into(),
        prompt: req.asked.as_deref().unwrap_or(&req.prompt).trim().to_string(),
        model_id: Some(m.id.clone()),
        mime: mime.into(),
        width: w,
        height: h,
        seconds: frames as f64 / fps as f64,
        parent,
        chat_id: req.chat_id.clone(),
        seed: Some(seed),
        poster: still.is_some(),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &bytes, still.as_deref())?])
}

/// A clip that came with its own sound (LTX): the engine puts it in the
/// WebM as raw PCM, which the window's player can't play, so it's made an
/// MP4 with AAC. A clip whose sound didn't come out (the engine then writes
/// a silent one) is kept as it is, and the log says so.
async fn with_its_sound(ctx: &RunCtx<'_>, webm: &std::path::Path, work: &std::path::Path) -> Result<(PathBuf, &'static str), String> {
    let state = ctx.state;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let probe = ffmpeg::probe(&ffprobe, webm).await?;
    if !probe.audio {
        state.log("media", "The video model's sound didn't come out; the clip is silent");
        return Ok((webm.to_path_buf(), "video/webm"));
    }
    (ctx.progress)("Putting the sound with it", 0.97);
    let mp4 = work.join("with-sound.mp4");
    let mut args: Vec<std::ffi::OsString> = vec!["-i".into(), webm.as_os_str().to_owned()];
    args.extend(ffmpeg::mp4_out(&mp4));
    ffmpeg::run(state, &ffmpeg_exe, args, probe.seconds, ctx.cancel, &|f| (ctx.progress)("Putting the sound with it", 0.97 + 0.03 * f)).await?;
    Ok((mp4, "video/mp4"))
}

struct LongClip<'a> {
    prompt: &'a str,
    /// Frames for each part.
    parts: &'a [u32],
    /// Frames in the finished clip.
    total: u32,
    fps: u32,
    w: u32,
    h: u32,
    seed: i64,
    /// The picture it starts from, if any.
    init: Option<PathBuf>,
    parent: Option<String>,
    still: Option<Vec<u8>>,
}

/// A clip longer than the model makes at once: part after part, each from
/// the last frame of the one before (with a rest when the PC runs hot),
/// then joined into one MP4.
async fn long_video(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path, c: LongClip<'_>) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let n = c.parts.len();
    let mut start = c.init.clone();
    let mut made: Vec<Vec<PathBuf>> = Vec::new();
    for (i, frames) in c.parts.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        if i > 0 {
            crate::perf::cool_down(&ctx.state.limits(), |what| (ctx.progress)(&format!("Letting the {what} cool down before part {}", i + 1), i as f64 / n as f64)).await;
        }
        // The engine reports each of its stages from 0 to 1; across the whole
        // clip, loading counts as the part's start and creating as most of it
        // (loading at 100% read as half the clip done, "10 s left").
        let stage = |s: &str, f: f64| {
            let f = f.clamp(0.0, 1.0);
            let within = match s {
                "Loading the model" => 0.0,
                "Creating" => f * 0.92,
                _ => 0.92 + f * 0.08,
            };
            (ctx.progress)(&format!("Part {} of {n}: {s}", i + 1), ((i as f64 + within) / n as f64) * 0.97)
        };
        let part_ctx = RunCtx { state: ctx.state, req, cancel: ctx.cancel, progress: &stage, reserved: ctx.reserved };
        let dir = work.join(format!("part{i:03}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let out = dir.join("f_%04d.png");
        let args = sd::clip_args(
            &ctx.files(m),
            &m.defaults,
            &sd::Clip {
                prompt: c.prompt,
                negative: req.negative.as_deref(),
                width: c.w,
                height: c.h,
                frames: *frames,
                fps: c.fps,
                steps: req.steps.unwrap_or(m.defaults.steps).clamp(4, 50),
                seed: c.seed + i as i64,
                init: start.as_deref(),
                audio: None,
                out: &out,
            },
            &part_ctx.limits(),
        );
        part_ctx.run_sd(args, &dir).await?;
        let files = longclip::frames_in(&dir, "f_");
        let last = files.last().cloned().ok_or("The video engine didn't produce any frames.")?;
        crate::applog::line("media", &format!("Long clip: part {} of {n} made ({} frames)", i + 1, files.len()));
        start = Some(last);
        made.push(files);
    }
    (ctx.progress)("Joining the parts into one video", 0.97);
    let mp4 = work.join("clip.mp4");
    let (bytes, first) = tokio::task::block_in_place(|| longclip::join(&made, c.total as usize, c.fps, &mp4, ctx.cancel, &|f| (ctx.progress)("Joining the parts into one video", 0.97 + f * 0.03)))?;
    let thumb = match c.still {
        Some(s) => s,
        None => imaging::thumbnail(&first)?,
    };
    let cipher = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "video".into(),
        op: "video".into(),
        prompt: req.asked.as_deref().unwrap_or(&req.prompt).trim().to_string(),
        model_id: Some(m.id.clone()),
        mime: "video/mp4".into(),
        width: first.width() / 2 * 2,
        height: first.height() / 2 * 2,
        seconds: (c.total.saturating_sub(1)) as f64 / c.fps as f64,
        parent: c.parent,
        chat_id: req.chat_id.clone(),
        seed: Some(c.seed),
        poster: true,
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &cipher, item, &bytes, Some(&thumb))?])
}

/// Caption lines from what was heard: a long stretch is split by words,
/// in proportion to its time, so no line runs past about 7 s or 84 letters.
pub(crate) fn caption_lines(segments: &[crate::speech::Segment]) -> Vec<(f64, f64, String)> {
    let mut out = Vec::new();
    for s in segments {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        let len = (s.end - s.start).max(0.3);
        let pieces = ((text.chars().count() as f64 / 84.0).ceil().max((len / 7.0).ceil())).max(1.0) as usize;
        let per = words.len().div_ceil(pieces).max(1);
        let total = words.len().max(1) as f64;
        for (i, chunk) in words.chunks(per).enumerate() {
            let a = s.start + len * (i * per) as f64 / total;
            let b = s.start + len * ((i * per + chunk.len()) as f64 / total);
            out.push((a, b, chunk.join(" ")));
        }
    }
    out
}

/// "00:01:02,345" for a subtitle file.
fn srt_time(secs: f64) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

pub(crate) fn srt(lines: &[(f64, f64, String)]) -> String {
    lines.iter().enumerate().map(|(i, (a, b, t))| format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(*a), srt_time(*b), t)).collect()
}

/// A file's path as FFmpeg's filters read it ("C\:/Users/…": the drive's
/// colon escaped, forward slashes).
fn filter_path(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "\\'")
}

/// A video with what's said in it as captions, in its own language or
/// translated (`language`): heard by whisper, cut into lines a viewer can
/// read, and drawn onto a new copy. The captions' text is kept with it.
async fn captions(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.clone().ok_or("Pick a video first.")?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let video = written_out(state, &c, &item, &work.join("source"))?;
    let probe = ffmpeg::probe(&ffprobe, &video).await?;
    if !probe.audio {
        return Err("This video has no sound, so there's nothing to caption.".into());
    }
    (ctx.progress)("Listening to what's said", 0.05);
    let mix = ffmpeg_stereo(state, &ffmpeg_exe, &video, probe.seconds, ctx.cancel).await?;
    let speech = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.map_err(|e| format!("Captions need a speech model to hear what's said. {e}"))?;
    let options = crate::speech::Options { language: Some("auto".into()), ..Default::default() };
    let heard = crate::speech::transcribe(&speech, &stems::resample(&mix.mono(), stems::RATE, crate::audio::RATE), &options).await?;
    // Whole sentences (whisper breaks mid-sentence), translated as wholes,
    // then cut into lines: cut first, "...the green pigment chlorophyll in
    // its" and "leaves captures sunlight" were translated apart.
    let mut sentences: Vec<crate::speech::Segment> =
        dub_lines(crate::speech::clean(heard).segments).into_iter().map(|(start, end, text)| crate::speech::Segment { start, end, text }).collect();
    if sentences.is_empty() {
        return Err("No one speaks in this video, so there's nothing to caption.".into());
    }
    let to = ctx.req.language.clone().filter(|l| !l.is_empty() && l != "auto" && l != "same");
    if let Some(to) = &to {
        (ctx.progress)("Translating the captions", 0.35);
        let said: Vec<String> = sentences.iter().map(|s| s.text.clone()).collect();
        let (ep, _, _hold) = crate::background_endpoint(state).await.map_err(|e| format!("Translating the captions needs a chat model: {e}"))?;
        let words = crate::translate::translate_lines(&ep, &said, to).await?;
        for (s, w) in sentences.iter_mut().zip(words) {
            s.text = w;
        }
    }
    let lines = caption_lines(&sentences);
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let text = srt(&lines);
    let file = work.join("captions.srt");
    std::fs::write(&file, &text).map_err(|e| e.to_string())?;
    // Big enough to read on a phone, white with a dark edge, near the bottom.
    let style = "FontName=Arial,FontSize=15,PrimaryColour=&H00FFFFFF,OutlineColour=&H00000000,BorderStyle=1,Outline=1.6,Shadow=0,MarginV=22";
    let out = work.join("captioned.mp4");
    let mut args: Vec<std::ffi::OsString> = ["-i".into(), video.as_os_str().to_owned()].into();
    args.extend(["-vf".into(), format!("subtitles='{}':force_style='{style}'", filter_path(&file)).into(), "-map".into(), "0:v".into(), "-map".into(), "0:a?".into()]);
    args.extend(ffmpeg::mp4_out(&out));
    (ctx.progress)("Drawing the captions", 0.5);
    ffmpeg::run(state, &ffmpeg_exe, args, probe.seconds, ctx.cancel, &|f| (ctx.progress)("Drawing the captions", 0.5 + 0.45 * f)).await?;
    let still = work.join("still.jpg");
    let _ = ffmpeg::run(state, &ffmpeg_exe, ["-ss", &format!("{:.2}", (probe.seconds / 3.0).min(2.0)), "-i"].iter().map(Into::into).chain([out.as_os_str().to_owned(), "-frames:v".into(), "1".into(), "-q:v".into(), "3".into(), still.as_os_str().to_owned()]).collect(), 0.0, ctx.cancel, &|_| {}).await;
    let thumb = std::fs::read(&still).ok().and_then(|b| imaging::decode(&b).ok()).and_then(|img| imaging::thumbnail(&img).ok());
    let bytes = std::fs::read(&out).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let made = MediaItem {
        kind: "video".into(),
        op: "captions".into(),
        prompt: item.prompt.clone(),
        model_id: item.model_id.clone(),
        mime: "video/mp4".into(),
        seconds: probe.seconds,
        width: probe.width,
        height: probe.height,
        parent: Some(item.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| item.chat_id.clone()),
        lyrics: Some(text),
        poster: thumb.is_some(),
        ..Default::default()
    };
    state.log("media", &format!("Captioned a video on this PC ({} lines{})", lines.len(), to.as_deref().map(|l| format!(", in {}", crate::translate::language_name(l))).unwrap_or_default()));
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, made, &bytes, thumb.as_deref())?])
}

/// Frames in each part of a talking picture: Wan 2.2 S2V makes about 5 s at
/// once (77 frames at its 16 a second).
const SPEAK_FRAMES: u32 = 77;
/// The longest voice a picture talks or sings to (each 5 s takes minutes).
const SPEAK_SECS: f64 = 60.0;

/// The parts of a mix HTDemucs was asked for (`wanted`: 0 drums, 1 bass,
/// 2 the rest, 3 vocals), added together; None when the separator isn't
/// there or fails (said in the log), so the caller makes do without.
async fn separated(ctx: &RunCtx<'_>, mix: &stems::Stereo, wanted: &[usize], why: &str) -> Result<Option<[Vec<f32>; 2]>, String> {
    let state = ctx.state;
    let Some(sep) = state.catalog.media.separator.clone() else { return Ok(None) };
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    let threads = state.limits().threads;
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let apart = tokio::task::block_in_place(|| {
        crate::cardfirst::asking(ask, || {
            let r = stems::separate(&dll, &separator, mix, threads, wanted, card, ctx.cancel);
            stems::unload();
            r
        })
    });
    match apart {
        Ok((parts, _)) => Ok(Some([0, 1].map(|ch| (0..mix.len()).map(|s| parts.iter().map(|p| p.ch[ch].get(s).copied().unwrap_or(0.0)).sum::<f32>()).collect::<Vec<f32>>()))),
        Err(e) if e == download::CANCELLED => Err(e),
        Err(e) => {
            state.log("media", &format!("The part separator couldn't take the song apart ({e}); {why}"));
            Ok(None)
        }
    }
}

/// A picture that talks or sings (Wan 2.2 S2V): its face and lips move to
/// a voice, 5 s at a time (each part starting from the last frame of the
/// one before), then FFmpeg joins the frames and lays the sound under
/// them. The voice is a sound or song from the gallery (a song's lips
/// follow its vocals alone), a file, or words read aloud first.
async fn speak(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let req = ctx.req;
    let c = state.work_cipher()?;
    let (picture, img) = ctx.source()?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let said = req.say.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ToSay { sound, secs, voice, song } = what_it_says(ctx, work, &ffmpeg_exe, &ffprobe, SPEAK_SECS).await?;

    // Its picture, at the model's size in its own shape.
    let d = &m.defaults;
    let fps = 16;
    let (iw, ih) = img.dimensions();
    let k = ((d.width.max(256) * d.height.max(256)) as f64 / (iw as f64 * ih as f64)).sqrt();
    let (w, h) = (imaging::round16(iw as f64 * k).clamp(256, 1280), imaging::round16(ih as f64 * k).clamp(256, 1280));
    let first = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let first_path = work.join("start.png");
    std::fs::write(&first_path, imaging::png(&first)?).map_err(|e| e.to_string())?;
    let still = imaging::thumbnail(&first)?;
    let prompt = match req.prompt.trim() {
        "" if song => "A person singing with feeling, lips moving with the words, natural expressions and slight head movement.",
        "" => "A person talking naturally, lips moving with the words, natural expressions and slight head movement.",
        p => p,
    };
    let seed = req.seed.unwrap_or_else(random_seed);
    let total = (secs * fps as f64).round() as u32 + 1;
    let parts = longclip::plan(total, SPEAK_FRAMES);
    let n = parts.len();
    let sr = stems::RATE as f64;
    let mut start = Some(first_path);
    let mut made: Vec<Vec<PathBuf>> = Vec::new();
    let mut at = 0u32;
    for (i, frames) in parts.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        if i > 0 {
            crate::perf::cool_down(&state.limits(), |what| (ctx.progress)(&format!("Letting the {what} cool down before part {}", i + 1), 0.05 + 0.9 * i as f64 / n as f64)).await;
        }
        // This part's stretch of the voice (the frame it starts on is the
        // last one of the part before).
        let from = ((at as f64 / fps as f64) * sr) as usize;
        let to = (((at + frames) as f64 / fps as f64) * sr) as usize;
        let piece = voice.get(from.min(voice.len())..to.min(voice.len())).unwrap_or_default();
        let wav = work.join(format!("voice{i:03}.wav"));
        std::fs::write(&wav, crate::audio::wav_encode(piece, stems::RATE)).map_err(|e| e.to_string())?;
        let stage = |s: &str, f: f64| {
            let within = match s {
                "Loading the model" => 0.0,
                "Creating" => f.clamp(0.0, 1.0) * 0.92,
                _ => 0.92 + f.clamp(0.0, 1.0) * 0.08,
            };
            let label = if n > 1 { format!("Part {} of {n}: {s}", i + 1) } else { s.to_string() };
            (ctx.progress)(&label, 0.05 + 0.9 * (i as f64 + within) / n as f64)
        };
        let part_ctx = RunCtx { state, req, cancel: ctx.cancel, progress: &stage, reserved: ctx.reserved };
        let dir = work.join(format!("part{i:03}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let out = dir.join("f_%04d.png");
        // Weights streamed to the card, without the engine's own memory cap (`streamed_only`).
        let args = without_vram(&sd::clip_args(
            &ctx.files(m),
            d,
            &sd::Clip { prompt, negative: req.negative.as_deref(), width: w, height: h, frames: *frames, fps, steps: req.steps.unwrap_or(d.steps).clamp(2, 50), seed: seed + i as i64, init: start.as_deref(), audio: Some(&wav), out: &out },
            &part_ctx.limits(),
        ));
        part_ctx.run_sd(args, &dir).await?;
        let files = longclip::frames_in(&dir, "f_");
        start = Some(files.last().cloned().ok_or("The video engine didn't produce any frames.")?);
        crate::applog::line("media", &format!("Talking picture: part {} of {n} made ({} frames)", i + 1, files.len()));
        made.push(files);
        at += frames - 1;
    }

    // The frames in order (each later part's first is the last one before), then the sound under them.
    (ctx.progress)("Putting the sound with it", 0.96);
    let seq = work.join("seq");
    std::fs::create_dir_all(&seq).map_err(|e| e.to_string())?;
    let mut k = 0usize;
    for (i, part) in made.iter().enumerate() {
        for f in part.iter().skip(usize::from(i > 0)) {
            if k >= total as usize {
                break;
            }
            let to = seq.join(format!("{k:05}.png"));
            std::fs::hard_link(f, &to).or_else(|_| std::fs::copy(f, &to).map(|_| ())).map_err(|e| e.to_string())?;
            k += 1;
        }
    }
    let mp4 = work.join("talking.mp4");
    let mut args: Vec<std::ffi::OsString> = ["-framerate".into(), fps.to_string().into(), "-i".into(), seq.join("%05d.png").into_os_string(), "-i".into(), sound.as_os_str().to_owned()].into();
    // S2V makes 16 frames a second, which looked choppy: frames made in
    // between from the motion around them double it (a 6 s clip in about 4 s).
    args.extend(["-vf", "minterpolate=fps=32:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1"].map(Into::into));
    args.extend(["-map", "0:v", "-map", "1:a", "-t", &format!("{secs:.3}")].map(Into::into));
    args.extend(ffmpeg::mp4_out(&mp4));
    ffmpeg::run(state, &ffmpeg_exe, args, secs, ctx.cancel, &|f| (ctx.progress)("Putting the sound with it", 0.96 + 0.04 * f)).await?;
    let bytes = std::fs::read(&mp4).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let item = MediaItem {
        kind: "video".into(),
        op: "speak".into(),
        prompt: said.map(str::to_string).unwrap_or_else(|| req.prompt.trim().to_string()),
        model_id: Some(m.id.clone()),
        mime: "video/mp4".into(),
        width: w,
        height: h,
        seconds: secs,
        parent: Some(picture.id),
        chat_id: req.chat_id.clone(),
        seed: Some(seed),
        poster: true,
        ..Default::default()
    };
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &bytes, Some(&still))?])
}

/// What a picture says or sings: the sound (as given), how long it is (at
/// most `longest`), and the voice alone at `stems::RATE`.
struct ToSay {
    sound: PathBuf,
    secs: f64,
    voice: Vec<f32>,
    song: bool,
}

/// The voice a picture talks or sings to: a sound or song from the gallery
/// (a song's lips follow its vocals alone), a file, or words read aloud first.
async fn what_it_says(ctx: &RunCtx<'_>, work: &std::path::Path, ffmpeg_exe: &std::path::Path, ffprobe: &std::path::Path, longest: f64) -> Result<ToSay, String> {
    let state = ctx.state;
    let req = ctx.req;
    let c = state.work_cipher()?;
    let said = req.say.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let mut song = false;
    let sound = match (&req.sound, &req.sound_file, said) {
        (Some(id), _, _) => {
            let a = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That sound is no longer in the gallery.")?;
            song = matches!(a.op.as_str(), "music" | "remake" | "revoice" | "studio_part");
            written_out(state, &c, &a, &work.join("sound"))?
        }
        (None, Some(f), _) => PathBuf::from(f),
        (None, None, Some(words)) => {
            (ctx.progress)("Reading the words aloud", 0.0);
            let text = crate::tts::speakable(words);
            let language = req.language.clone().filter(|l| !l.is_empty() && l != "auto");
            let (floats, rate) = match req.voice.as_deref().and_then(|v| v.strip_prefix("own:")) {
                Some(own) => own_voice::speak(state, own, &text, language.as_deref(), ctx.cancel, &|s, f| (ctx.progress)(s, 0.03 * f)).await?,
                None => tokio::task::block_in_place(|| -> Result<(Vec<f32>, u32), String> {
                    let voices = crate::tts::voices()?;
                    let voice = crate::tts::pick_voice(&voices, req.voice.as_deref(), language.as_deref()).map(|v| v.id.clone()).ok_or("No voices are installed.")?;
                    let s = crate::tts::synthesize(&text, Some(&voice), language.as_deref(), 1.0)?;
                    Ok((s.samples.iter().map(|x| *x as f32 / 32768.0).collect(), s.rate))
                })?,
            };
            let p = work.join("sound.wav");
            std::fs::write(&p, crate::audio::wav_encode(&floats, rate)).map_err(|e| e.to_string())?;
            p
        }
        _ => return Err("Pick what it should say or sing: a sound or song, a file, or words to read aloud.".into()),
    };
    let probe = ffmpeg::probe(ffprobe, &sound).await?;
    if !probe.audio {
        return Err("That file has no sound to talk to.".into());
    }
    let secs = probe.seconds.clamp(0.5, longest);
    let mix = ffmpeg_stereo(state, ffmpeg_exe, &sound, secs, ctx.cancel).await?;
    // A song's lips follow its singing, not the band.
    let voice = match song {
        true => {
            (ctx.progress)("Finding the singing in the song", 0.03);
            separated(ctx, &mix, &[3], "the lips follow the whole song").await?.map(|[l, r]| l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect())
        }
        false => None,
    }
    .unwrap_or_else(|| mix.mono());
    Ok(ToSay { sound, secs, voice, song })
}

/// A picture that talks or sings, the quick way (`talk`, Ditto): its face
/// and lips move to the voice at 25 frames a second in one go, then FFmpeg
/// lays the sound under the video. Saved as a talking picture (`speak`).
#[cfg(windows)]
async fn talk(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let req = ctx.req;
    let d = state.catalog.media.ditto.clone().ok_or("Quick talking pictures aren't available on this system yet.")?;
    let c = state.work_cipher()?;
    let (picture, img) = ctx.source()?;
    let (ffmpeg_exe, ffprobe) = ffmpeg::ensure(state, ctx.cancel, ctx.progress).await?;
    let said = req.say.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ToSay { sound, secs, voice, .. } = what_it_says(ctx, work, &ffmpeg_exe, &ffprobe, ditto::LONGEST).await?;
    let uv = ensure_ditto(ctx, &d).await?;

    // The picture as it is (at most 1920 on its longest side), and the voice alone.
    let (w, h) = ditto::picture_size(img.width(), img.height());
    let first = if (w, h) == img.dimensions() { img.clone() } else { img.resize_exact(w, h, image::imageops::FilterType::Lanczos3) };
    let image = work.join("face.png");
    std::fs::write(&image, imaging::png(&first)?).map_err(|e| e.to_string())?;
    let still = imaging::thumbnail(&first)?;
    let wav = work.join("voice.wav");
    std::fs::write(&wav, crate::audio::wav_encode(&voice, stems::RATE)).map_err(|e| e.to_string())?;
    let silent = work.join("silent.mp4");

    (ctx.progress)("Loading the face model", 0.1);
    let mut frames = (secs * 25.0).ceil().max(1.0) as u32;
    let mut no_face = false;
    let ran = proc::run_env(
        proc::Run {
            exe: &uv,
            args: ditto::args(&ditto::source_dir(&state.paths, &d), &ditto::models_dir(&state.paths), &image, &wav, &silent, req.steps.unwrap_or(50)),
            cwd: work,
            log: &state.paths.logs.join("talking-picture.log"),
            low_priority: state.limits().low_priority,
            job: state.job(),
        },
        &ditto::env(&state.paths),
        ctx.cancel,
        |l| match ditto::said(l) {
            Some(ditto::Said::Loaded) => (ctx.progress)("Moving the face to the voice", 0.15),
            Some(ditto::Said::Frames(n)) => frames = n.max(1),
            Some(ditto::Said::Frame(k)) => (ctx.progress)(&format!("Making the frames: {k} of {frames}"), 0.15 + 0.78 * (k as f64 / frames as f64).min(1.0)),
            Some(ditto::Said::NoFace) => no_face = true,
            None => {}
        },
    )
    .await;
    if no_face {
        return Err("No face was found in the picture. Pick one where a face is clearly seen from the front.".into());
    }
    ran?;

    (ctx.progress)("Putting the sound with it", 0.94);
    let mp4 = work.join("talking.mp4");
    let mut args: Vec<std::ffi::OsString> = vec!["-i".into(), silent.into_os_string(), "-i".into(), sound.as_os_str().to_owned()];
    args.extend(["-map", "0:v", "-map", "1:a", "-t", &format!("{secs:.3}"), "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"].map(Into::into));
    args.push(mp4.as_os_str().to_owned());
    ffmpeg::run(state, &ffmpeg_exe, args, secs, ctx.cancel, &|f| (ctx.progress)("Putting the sound with it", 0.94 + 0.06 * f)).await?;
    let bytes = std::fs::read(&mp4).map_err(|e| format!("FFmpeg didn't make the video: {e}"))?;
    let item = MediaItem {
        kind: "video".into(),
        op: "speak".into(),
        prompt: said.map(str::to_string).unwrap_or_else(|| req.prompt.trim().to_string()),
        model_id: Some("ditto".into()),
        mime: "video/mp4".into(),
        width: w,
        height: h,
        seconds: secs,
        parent: Some(picture.id),
        chat_id: req.chat_id.clone(),
        poster: true,
        ..Default::default()
    };
    state.log("media", &format!("Made a picture talk the quick way on this PC ({secs:.0} s)"));
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &bytes, Some(&still))?])
}

/// Ditto, set up the first time (`talk`): uv (the music trainer's), its
/// source with SulcusAI's runner, its locked Python packages, and its model
/// files. Returns uv.
#[cfg(windows)]
async fn ensure_ditto(ctx: &RunCtx<'_>, d: &ditto::DittoCatalog) -> Result<PathBuf, String> {
    let state = ctx.state;
    let paths = &state.paths;
    let say = "Getting the quick face model (once)";
    let uv = engine::ensure_unpacked(paths, "trainer-uv-bin", &d.uv, "uv.exe", ctx.cancel, |r, all| (ctx.progress)(say, 0.01 * r as f64 / all.max(1) as f64)).await?;
    let source = ditto::source_dir(paths, d);
    let dir_name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
    engine::ensure_unpacked(paths, &dir_name, &d.source, "talk.py", ctx.cancel, |_, _| {}).await?;
    if !source.join(".venv").join("Scripts").join("python.exe").is_file() {
        // Its Python and packages (PyTorch for NVIDIA cards and the rest), as locked.
        state.log("network", "Downloading the quick face model's Python packages from pypi.org and download.pytorch.org");
        (ctx.progress)("Setting up the quick face model (once, up to 6 GB)", 0.02);
        let ran = proc::run_env(
            proc::Run {
                exe: &uv,
                args: ["sync", "--frozen", "--python", "3.12"].map(std::ffi::OsString::from).to_vec(),
                cwd: &source,
                log: &paths.logs.join("talking-picture-setup.log"),
                low_priority: state.limits().low_priority,
                job: state.job(),
            },
            &ditto::env(paths),
            ctx.cancel,
            |_| {},
        )
        .await;
        if let Err(e) = ran {
            // A half-made environment would look ready next time.
            let _ = std::fs::remove_dir_all(source.join(".venv"));
            return Err(e);
        }
    }
    let dir = ditto::models_dir(paths);
    let missing: Vec<&train::TrainerFile> = d.files.iter().filter(|f| !dir.join(&f.file).is_file()).collect();
    if !missing.is_empty() {
        state.log("network", "Downloading Ditto's model files from huggingface.co");
        let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false)?;
        let total: u64 = missing.iter().map(|f| f.size).sum::<u64>().max(1);
        let mut done = 0u64;
        for f in missing {
            let base = done;
            download::fetch_verified(&client, &f.url, &dir.join(&f.file), f.size, &f.sha256, ctx.cancel, |r, _| (ctx.progress)(say, 0.03 + 0.06 * (base + r) as f64 / total as f64)).await?;
            done += f.size;
        }
    }
    Ok(uv)
}

/// The vocal language for the music model: the one asked for; with no
/// lyrics and none asked for, English (left to itself, the music model can
/// sing made-up words); else none, and it tells from the lyrics.
fn vocal_language(asked: Option<&str>, words: &lyrics::Words) -> Option<String> {
    match (asked.filter(|l| !l.is_empty() && *l != "auto"), words) {
        (_, lyrics::Words::Instrumental) => None,
        (Some(l), _) => Some(l.to_string()),
        // Always named: the music model sings more clearly told the language.
        (None, lyrics::Words::Sung(l)) => Some(lyrics::guess_language(l).to_string()),
        (None, lyrics::Words::MusicModelWrites) => Some("en".into()),
    }
}

/// The request with its saved artist applied (`MusicSpec::persona`), or
/// None when it names none. An artist that's gone is said plainly.
fn with_persona(state: &AppState, req: &Request) -> Result<Option<Request>, String> {
    let Some(name) = req.music.as_ref().and_then(|m| m.persona.as_deref()).map(str::trim).filter(|n| !n.is_empty()) else { return Ok(None) };
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let p = persona::find(&conn, &c, name).ok_or_else(|| format!("There's no saved artist called {name}."))?;
    let mut out = req.clone();
    let spec = out.music.get_or_insert_with(Default::default);
    p.apply(spec, &mut out.language, |id| store::get(&conn, &c, id).is_some_and(|i| i.kind == "audio"));
    // Their first song's seed: the same start keeps the voice closer.
    out.seed = out.seed.or(p.seed);
    Ok(Some(out))
}

/// The length of a new song set to Auto, when it's known before any lyrics
/// are written: one named in the description ("a 90-second jingle"), what
/// the person's own lyrics need, what the chosen sections take, or a
/// minute and a half for an instrumental. None otherwise: written lyrics
/// decide it.
fn auto_seconds(req: &Request) -> Option<f32> {
    let spec = req.music.as_ref().filter(|m| m.auto_length && matches!(m.task.as_deref(), None | Some("new")))?;
    if req.op != "music" || req.seconds.is_some() {
        return None;
    }
    let flow = spec.flow(&req.prompt);
    let own = req.lyrics.as_deref().filter(|l| lyrics::sung_lines(l) > 0).filter(|_| req.instrumental != Some(true));
    lyrics::named_length(&req.prompt)
        .or_else(|| own.map(|l| lyrics::length_of(l, flow)))
        .or_else(|| lyrics::short_piece(&req.prompt))
        .or_else(|| (!spec.sections().is_empty()).then(|| lyrics::length_of_sections(&spec.sections(), flow)))
        .or_else(|| (req.instrumental == Some(true)).then_some(90.0))
}

/// A new song in a recorded voice with no singer picked or described: sung
/// by a man or a woman as the recording is (left to itself the music model
/// often picked the other, and the voice was then changed across sexes).
fn with_own_voice_sex(state: &AppState, req: &Request) -> Option<Request> {
    let spec = req.music.as_ref().filter(|m| req.op == "music" && matches!(m.task.as_deref(), None | Some("new")))?;
    let id = spec.own_voice.as_deref()?;
    if MusicSpec::some(&spec.voice).is_some() || names_a_singer(&req.prompt) {
        return None;
    }
    let c = state.work_cipher().ok()?;
    let voice = own_voice::get(&state.db.lock().unwrap(), &c, id)?;
    let sex = own_voice::sex_of(&state.paths, &c, &voice)?;
    crate::applog::line("media", &format!("A song in a recorded voice: sung by a {} singer, as the recording is", sex));
    let mut out = req.clone();
    out.music.as_mut()?.voice = Some(sex.into());
    Some(out)
}

/// Whether a description already says who sings ("a female singer", "a boy
/// band", "a duet").
fn names_a_singer(prompt: &str) -> bool {
    const WHO: &[&str] = &["female", "male", "woman", "women", "man", "men", "girl", "girls", "boy", "boys", "duet", "choir", "baritone", "tenor", "alto", "soprano", "mezzo"];
    prompt.to_lowercase().split(|c: char| !c.is_alphabetic()).any(|w| WHO.contains(&w))
}

/// Whether a song job wants lyrics written before it starts: a new song
/// with vocals and none given, or vocals added to a song the person has.
fn wants_written_lyrics(req: &Request) -> bool {
    let spec = req.music.as_ref();
    let task = spec.and_then(|m| m.task.as_deref()).filter(|t| *t != "new");
    let sings = match task {
        None => true,
        Some("add") => spec.and_then(|m| m.track.as_deref()).is_some_and(|t| t.contains("vocals")),
        _ => false,
    };
    req.op == "music" && sings && lyrics::words(false, req.instrumental == Some(true), req.lyrics.as_deref()) == lyrics::Words::MusicModelWrites
}

/// What the lyrics writer and the music model are told the song is.
fn song_caption(req: &Request) -> String {
    let instrumental = req.instrumental == Some(true);
    match &req.music {
        Some(m) => m.caption(&req.prompt, instrumental),
        None => req.prompt.trim().to_string(),
    }
}

async fn song(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let sound = req.op == "sound";
    let spec = req.music.clone().unwrap_or_default();
    let task = spec.task.as_deref().filter(|t| *t != "new" && !sound);
    let c = ctx.state.work_cipher()?;
    // A song from the gallery as a file the engine reads, with its length.
    let audio_file = |id: &str, name: &str| -> Result<(MediaItem, std::path::PathBuf, f64), String> {
        let item = store::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
        if item.kind != "audio" {
            return Err("That isn't a song or a sound.".into());
        }
        let bytes = store::read(&ctx.state.paths, &c, id)?;
        let dir = work.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let (path, secs) = audio_in::for_engine(&bytes, &item.mime, &dir)?;
        Ok((item.clone(), path, if secs > 0.0 { secs } else { item.seconds }))
    };
    let source = match (task, spec.audio.as_deref()) {
        (Some(_), Some(id)) => Some(audio_file(id, "source")?),
        (Some(_), None) => return Err("Pick the song to work on first.".into()),
        _ => None,
    };
    let reference = match spec.reference.as_deref() {
        Some(id) if !sound => Some(audio_file(id, "reference")?),
        _ => None,
    };
    // A new song in a recorded voice is made with the person's singing as
    // the music model's reference (it sets the voice's sound and delivery,
    // not the tune), so it starts out sung more like them.
    let own_reference = match spec.own_voice.as_deref() {
        Some(id) if reference.is_none() && task.is_none() && !sound && req.instrumental != Some(true) && m.file("yue").is_none() && m.file("heartmula").is_none() => own_reference(ctx, id, work),
        _ => None,
    };
    // A new song: as long as asked, up to ten minutes (sound effects 30 s).
    let seconds = if sound { req.seconds.unwrap_or(10.0).clamp(10.0, 30.0) } else { req.seconds.unwrap_or(m.defaults.seconds.max(30.0)).clamp(10.0, 600.0) };
    let source_len = source.as_ref().map_or(0.0, |s| s.2 as f32);
    let track = spec.track.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(|t| t.to_lowercase().replace(' ', "_"));
    let (engine_task, region) = match task {
        Some("cover") => ("cover", None),
        // The whole song when no part is given.
        Some("repaint") => ("repaint", Some(spec.region.map_or((0.0, -1.0), |r| (r[0].max(0.0), if r[1] > r[0] { r[1] } else { -1.0 })))),
        // More after its end: a part past the source's end is made new.
        Some("extend") => ("repaint", Some((source_len, source_len + spec.extend_by.unwrap_or(30.0).clamp(5.0, 300.0)))),
        Some("add") => ("lego", None),
        Some("extract") => ("extract", None),
        Some("complete") => ("complete", None),
        _ => ("text2music", None),
    };
    let caption_words = song_caption(req);
    let mut problem = req.lyrics_problem.clone();
    let sings = matches!(task, None) || (task == Some("add") && track.as_deref().is_some_and(|t| t.contains("vocals")));
    let mut words = if matches!(task, Some("extract" | "complete")) {
        lyrics::Words::Instrumental
    } else if sings {
        lyrics::words(sound, req.instrumental == Some(true), req.lyrics.as_deref())
    } else {
        // A cover, a part made again or more after the end sings the words
        // given, else the song's own (a cover of a song keeps its words).
        let own = source.as_ref().and_then(|s| s.0.lyrics.clone()).filter(|l| lyrics::has_singing(l));
        match req.lyrics.as_deref().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).or(own) {
            Some(l) => lyrics::Words::Sung(l),
            None if req.instrumental == Some(true) => lyrics::Words::Instrumental,
            None => lyrics::Words::MusicModelWrites,
        }
    };
    if sings && words == lyrics::Words::MusicModelWrites && problem.is_none() {
        // Not written before the job (it always is for the Studio's and chats' songs).
        (ctx.progress)("Writing lyrics", 0.02);
        let length = if task.is_some() { source_len.max(10.0) } else { seconds };
        match lyrics::write(ctx.state, &caption_words, Some(length), req.language.as_deref(), &spec.sections(), &spec.lyrics_guide(), spec.flow(&req.prompt)).await {
            Ok(l) => words = lyrics::Words::Sung(l),
            Err(why) => problem = Some(why),
        }
    }
    let caption = if sound {
        format!("{}. A sound effect recording, not music: no melody, no singing.", req.prompt.trim().trim_end_matches('.'))
    } else if caption_words.is_empty() {
        // Taking a part out, or building around one, needs no description.
        source.as_ref().map(|s| s.0.prompt.clone()).filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "music".into())
    } else {
        caption_words.clone()
    };
    // Directions written in the lyrics' brackets, Suno's way, go into the
    // description; the tags keep only a word or two (`lyrics::tidy`).
    let caption = match req.lyrics.as_deref().map(lyrics::directions_of).filter(|d| !d.is_empty() && !sound) {
        Some(d) => format!("{caption}, {}", d.join(", ")),
        None => caption,
    };
    // A custom model: its trigger word starts the description, as in training.
    let custom = match spec.custom.as_deref().filter(|c| !c.is_empty()) {
        Some(id) => Some(train::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That custom model is no longer here.")?),
        None => None,
    };
    let caption = match &custom {
        Some(cm) => format!("{}, {caption}", cm.trigger),
        None => caption,
    };
    let adapters = train::adapters_dir(&ctx.state.paths);
    let yue = m.file("yue").zip(m.file("yue_vae"));
    if yue.is_some() && (task.is_some() || sound) {
        return Err("YuE2 makes new songs. To work on a song or make a sound effect, pick an ACE-Step model.".into());
    }
    let heart = m.file("heartmula");
    if heart.is_some() && (task.is_some() || sound) {
        return Err("HeartMuLa makes new songs. To work on a song or make a sound effect, pick an ACE-Step model.".into());
    }
    let seed = req.seed.unwrap_or_else(random_seed);
    // Words to sing and nothing wrote them: said now, not after the whole
    // song (the music model's own lyrics came out as garbage).
    if sings && words == lyrics::Words::MusicModelWrites && !sound {
        return Err(lyrics::no_singing_message(problem.as_ref()));
    }
    // A new song gets its tempo, key and time signature (see `music::settle`).
    let (bpm, key, time_signature) = if task.is_none() {
        let (b, k, t) = music::settle(&caption, spec.bpm, spec.key.as_deref(), spec.time_signature.as_deref(), seed);
        (Some(b), Some(k), Some(t))
    } else {
        (spec.bpm, spec.key.clone(), spec.time_signature.clone())
    };
    let language = vocal_language(req.language.as_deref(), &words);
    let lyrics_text = if task.is_some() && words == lyrics::Words::MusicModelWrites {
        String::new()
    } else if matches!(words, lyrics::Words::Sung(_)) && task.is_none() {
        // A screamed chorus or a whispered verse is said on its tag too.
        spec.mark_sections(words.for_engine())
    } else {
        words.for_engine().to_string()
    };
    let models = model_dir(&ctx.state.paths, m);
    let versions = if task.is_none() && !sound { req.count.unwrap_or(1).clamp(1, 3) } else { 1 };
    let made = if let Some((backbone, vae)) = yue {
        let engine_dir = yue_engine_dir(ctx.state).ok_or("YuE2's engine isn't installed. Reinstall YuE2 from the Studio's Models list.")?;
        // YuE2 reads the language among its style tags.
        let style = match language.as_deref() {
            Some(l) => format!("{caption}, {}", crate::translate::language_name(l)),
            None => caption.clone(),
        };
        yue::make(
            &music::Engine { dir: &engine_dir, models: &models, low_priority: ctx.state.limits().low_priority, job: ctx.state.job(), logs: &ctx.state.paths.logs },
            work,
            &yue::YueRequest { style: &style, lyrics: &lyrics_text, seconds, seed, versions, backbone: &backbone.file, vae: &vae.file, weirdness: spec.weirdness },
            ctx.cancel,
            ctx.progress,
        )
        .await?
    } else if let Some(file) = heart {
        let engine_dir = heartmula_engine_dir(ctx.state).ok_or("HeartMuLa's engine isn't installed. Reinstall HeartMuLa from the Studio's Models list.")?;
        let mut made = heartmula::make(
            &music::Engine { dir: &engine_dir, models: &models, low_priority: ctx.state.limits().low_priority, job: ctx.state.job(), logs: &ctx.state.paths.logs },
            work,
            &heartmula::HeartRequest {
                caption: &caption,
                lyrics: &heartmula::lyrics_for(&lyrics_text),
                seconds,
                seed,
                versions,
                model: &file.file,
                // Weirdness and Style influence, their own at the middle.
                temperature: spec.weirdness.map(|w| 0.7 + 0.6 * w.clamp(0.0, 1.0)),
                guidance: spec.influence.map(|x| 1.0 + x.clamp(0.0, 1.0)),
            },
            ctx.cancel,
            ctx.progress,
        )
        .await?;
        for song in &mut made {
            song.lyrics = lyrics_text.clone();
        }
        made
    } else {
    // The planning model isn't used (see `music::make`); older installs still have it.
    let dit = m.file("dit").ok_or("The music model is missing a file.")?;
    let lm_file = m.file("lm").map(|f| f.file.clone()).unwrap_or_default();
    let engine_dir = music_engine_dir(ctx.state).ok_or("The music engine isn't installed. Reinstall the music model from the Studio's Models list.")?;
    music::make(
        &music::Engine {
            dir: &engine_dir,
            models: &models,
            low_priority: ctx.state.limits().low_priority,
            job: ctx.state.job(),
            logs: &ctx.state.paths.logs,
        },
        work,
        &music::SongRequest {
            caption: &caption,
            lyrics: &lyrics_text,
            seconds: if task.is_some() { source_len.max(10.0) } else { seconds },
            seed,
            language: language.as_deref(),
            lm_file: &lm_file,
            dit_file: &dit.file,
            bpm,
            key: key.as_deref(),
            time_signature: time_signature.as_deref(),
            task: engine_task,
            track: track.as_deref(),
            region,
            strength: spec.strength,
            source: source.as_ref().map(|s| s.1.as_path()),
            reference: reference.as_ref().map(|r| r.1.as_path()).or(own_reference.as_deref()),
            versions,
            guidance: music::guidance(&dit.file, spec.influence),
            negative: spec.exclude.as_deref(),
            adapters: custom.as_ref().map(|_| adapters.as_path()),
            adapter: custom.as_ref().map(|cm| cm.id.as_str()),
            adapter_scale: spec.custom_scale,
            ..Default::default()
        },
        ctx.cancel,
        ctx.progress,
    )
    .await?
    };
    if let Some(r) = &own_reference {
        let _ = std::fs::remove_file(r);
    }
    let planned = made[0].lyrics.clone();
    // Vocals were asked for, and the music model planned nothing to sing:
    // say so rather than hand back an instrumental as if it were the song.
    if task.is_none() {
        if let Err(e) = lyrics::check_sung(&words, &planned, problem.as_ref()) {
            crate::applog::line("media", &format!("The song came out without words to sing (planned lyrics: {:?})", planned.chars().take(80).collect::<String>()));
            return Err(e);
        }
    }
    // Harmonies layered on afterwards, by the model that adds parts: the
    // sung ones as backing vocals, the instrument's as its part.
    let mut made = made;
    let mut note = None;
    if task.is_none() && !sound && spec.harmony_layer {
        let mut layers: Vec<(&str, String)> = Vec::new();
        if req.instrumental != Some(true) {
            if let Some(h) = spec.harmony_vocals() {
                layers.push(("backing_vocals", h));
            }
        }
        if let (Some(part), Some(h)) = (spec.harmony_instrument.as_deref().and_then(part_for), spec.harmony_line()) {
            layers.push((part, h));
        }
        match (layers.is_empty(), parts_model(ctx.state)) {
            (true, _) => {}
            (false, None) => note = Some("The harmonies were made with the song: layering them on afterwards needs ACE-Step 1.5 Base (Models).".to_string()),
            (false, Some(base)) => {
                let base_dit = base.file("dit").ok_or("ACE-Step 1.5 Base is missing a file.")?;
                let base_models = model_dir(&ctx.state.paths, &base);
                let engine_dir = music_engine_dir(ctx.state).ok_or("The music engine isn't installed. Reinstall the music model from the Studio's Models list.")?;
                for (vi, song) in made.iter_mut().enumerate() {
                    for (li, (part, words)) in layers.iter().enumerate() {
                        (ctx.progress)(if *part == "backing_vocals" { "Layering the harmony vocals" } else { "Layering the harmony instrument" }, 0.0);
                        let dir = work.join(format!("layer-{vi}-{li}"));
                        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                        let (src, _) = audio_in::for_engine(&song.mp3, "audio/mpeg", &dir)?;
                        let layered = music::make(
                            &music::Engine {
                                dir: &engine_dir,
                                models: &base_models,
                                low_priority: ctx.state.limits().low_priority,
                                job: ctx.state.job(),
                                logs: &ctx.state.paths.logs,
                            },
                            &dir,
                            &music::SongRequest {
                                caption: &format!("{words}, {caption}"),
                                lyrics: &song.lyrics,
                                seconds: song.seconds as f32,
                                seed,
                                language: language.as_deref(),
                                lm_file: "",
                                dit_file: &base_dit.file,
                                bpm,
                                key: key.as_deref(),
                                time_signature: time_signature.as_deref(),
                                task: "lego",
                                track: Some(part),
                                source: Some(src.as_path()),
                                versions: 1,
                                guidance: music::guidance(&base_dit.file, None),
                                ..Default::default()
                            },
                            ctx.cancel,
                            ctx.progress,
                        )
                        .await?;
                        if let Some(l) = layered.into_iter().next() {
                            song.mp3 = l.mp3;
                        }
                    }
                }
                crate::applog::line("media", &format!("Layered {} harmony part(s) with {}", layers.len(), base.name));
            }
        }
    }
    // What it is, in the gallery: the person's words, else what was done to which song.
    let named = |s: &(MediaItem, std::path::PathBuf, f64)| if s.0.prompt.trim().is_empty() { "the song".to_string() } else { format!("“{}”", s.0.prompt.trim()) };
    let prompt = match (task, source.as_ref()) {
        (Some("extract"), Some(s)) => format!("The {} of {}", track.as_deref().unwrap_or("part").replace('_', " "), named(s)),
        (Some("add"), Some(s)) => format!("{} with {} added", named(s), track.as_deref().unwrap_or("a part").replace('_', " ")),
        (Some("complete"), Some(s)) => format!("{} made into a full song", named(s)),
        (Some("extend"), Some(s)) if req.prompt.trim().is_empty() => format!("{}, made longer", named(s)),
        (Some("repaint"), Some(s)) if req.prompt.trim().is_empty() => format!("{} with a part made again", named(s)),
        _ if req.prompt.trim().is_empty() => caption.clone(),
        _ => req.prompt.trim().to_string(),
    };
    let mut items = Vec::new();
    for song in made {
        let item = MediaItem {
            kind: "audio".into(),
            op: req.op.clone(),
            prompt: prompt.clone(),
            model_id: Some(m.id.clone()),
            mime: "audio/mpeg".into(),
            seconds: song.seconds,
            parent: source.as_ref().map(|s| s.0.id.clone()),
            chat_id: req.chat_id.clone(),
            seed: Some(seed),
            // A new song is named as asked, else by its hook; work on a song keeps its name.
            title: if task.is_none() {
                spec.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).or_else(|| (!sound).then(|| lyrics::title_of(&song.lyrics)).flatten())
            } else {
                source.as_ref().and_then(|s| s.0.title.clone())
            },
            lyrics: (!sound && lyrics::has_singing(&song.lyrics)).then_some(song.lyrics),
            // A new song's settings, to make it again (none of the title: a new take gets its own).
            music: (task.is_none() && !sound).then(|| MusicSpec { title: None, ..spec.clone() }),
            note: note.clone(),
            ..Default::default()
        };
        items.push(store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &song.mp3, None)?);
    }
    Ok(items)
}

/// A recorded voice's singing as a file for the music model, in the job's
/// work folder for the run only. None, said in the log, when there's no
/// singing (reading as the reference could make the song spoken) or it
/// can't be read: the song is then made as before.
fn own_reference(ctx: &RunCtx<'_>, id: &str, work: &std::path::Path) -> Option<PathBuf> {
    let c = ctx.state.work_cipher().ok()?;
    let voice = own_voice::get(&ctx.state.db.lock().unwrap(), &c, id)?;
    match own_voice::singing_reference(&ctx.state.paths, &c, &voice) {
        Ok((_, false)) => {
            crate::applog::line("media", "A song in a recorded voice: made without their voice as the reference (no singing recorded)");
            None
        }
        Ok((samples, true)) => {
            let path = work.join("own-voice.wav");
            std::fs::write(&path, crate::audio::wav_encode(&samples, svc::RATE)).ok()?;
            crate::applog::line("media", &format!("A song in a recorded voice: made with {:.0} s of their singing as the reference", samples.len() as f32 / svc::RATE as f32));
            Some(path)
        }
        Err(e) => {
            crate::applog::line("media", &format!("A song in a recorded voice: made without their voice as the reference ({e})"));
            None
        }
    }
}

/// ONNX Runtime and a helper model (`MediaCatalog::separator`,
/// `note_reader`), downloaded the first time, with `getting` shown meanwhile.
async fn ensure_helper(state: &Arc<AppState>, h: &Helper, getting: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, PathBuf), String> {
    let dll = crate::natural::ensure_runtime(state, cancel, &|_, _, _| {}).await?;
    Ok((dll, fetch_helper(state, h, getting, cancel, progress).await?))
}

/// The part separator for work outside a job (a recording added to a
/// voice): ONNX Runtime, HTDemucs, and the NVIDIA card when its runtime is
/// already here and loaded (it isn't downloaded for this; the processor
/// does it otherwise).
pub(crate) async fn separator(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, PathBuf, Option<crate::natural::Card>), String> {
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    let (dll, model) = ensure_helper(state, &sep, "Getting the part separator (once)", cancel, progress).await?;
    // Short work: CUDA when its part is here; the processor otherwise (the
    // separator can't run through DirectML).
    let ready = crate::natural::card_runtime_dll(&state.paths, &state.catalog.voices).is_some() && !crate::natural::loaded_for_processor();
    let card = if ready { crate::natural::pick_card(state, false).await.ok().filter(|p| !p.needs_cuda && !p.card.dml).map(|p| p.card) } else { None };
    Ok((dll, model, card))
}

/// A helper's file, downloaded the first time.
async fn fetch_helper(state: &Arc<AppState>, h: &Helper, getting: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<PathBuf, String> {
    let dest = state.paths.models.join("media").join("helpers").join(&h.file.file);
    if !dest.metadata().is_ok_and(|m| m.len() == h.file.size) {
        progress(getting, 0.0);
        state.log("network", &format!("Downloading {} ({}) from {}", h.file.file, crate::size_label(h.file.size), crate::host_of(&h.file.url)));
        let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        download::fetch_verified(&client, &h.file.url, &dest, h.file.size, &h.file.sha256, cancel, |r, t| progress(getting, r as f64 / t.max(1) as f64)).await?;
    }
    Ok(dest)
}

/// An installed music model that adds parts to a song (ACE-Step 1.5 Base).
fn parts_model(state: &AppState) -> Option<MediaModel> {
    let have: Vec<String> = installed(&state.db.lock().unwrap()).into_iter().map(|i| i.model_id).collect();
    models(state).into_iter().find(|m| m.kind == Kind::Music && m.can("parts") && have.contains(&m.id))
}

/// A song from the gallery as stereo at the part separator's rate.
fn decoded_stereo(bytes: &[u8], mime: &str) -> Result<stems::Stereo, String> {
    let dir = std::env::temp_dir().join(format!("sulcusai-song-{}", store::new_id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(if mime.contains("wav") { "song.wav" } else { "song.mp3" });
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let decoded = audio_in::decode(&path);
    let _ = std::fs::remove_dir_all(&dir);
    let (samples, rate, channels) = decoded?;
    let ch = channels.max(1) as usize;
    let left: Vec<f32> = samples.chunks(ch).map(|f| f[0]).collect();
    let right: Vec<f32> = samples.chunks(ch).map(|f| f[if ch > 1 { 1 } else { 0 }]).collect();
    Ok(stems::Stereo { rate, ch: [left, right] }.resampled(stems::RATE))
}

/// A song's parts as songs of their own under it (Suno's Get stems): its
/// vocals and the instrumental, or (`MusicSpec::track` "all") the vocals,
/// drums, bass and the rest, taken apart by HTDemucs.
async fn stems_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let song = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Only a song or a sound has parts.".into());
    }
    let every = ctx.req.music.as_ref().and_then(|m| m.track.as_deref()) == Some("all");
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Taking the parts apart", 0.1);
    let bytes = store::read(&state.paths, &c, &song.id)?;
    let threads = state.limits().threads;
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let parts = tokio::task::block_in_place(|| crate::cardfirst::asking(ask, || -> Result<Vec<stems::Stereo>, String> {
        let mix = decoded_stereo(&bytes, &song.mime)?;
        let (parts, _) = stems::separate(&dll, &separator, &mix, threads, &[0, 1, 2, 3], card, ctx.cancel)?;
        stems::unload();
        Ok(parts)
    }))?;
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    // HTDemucs gives the drums, bass, the rest and the vocals, in that order.
    let wanted: Vec<(&str, Vec<usize>)> = if every { vec![("vocals", vec![3]), ("drums", vec![0]), ("bass", vec![1]), ("other instruments", vec![2])] } else { vec![("vocals", vec![3]), ("instrumental", vec![0, 1, 2])] };
    let name = song.title.clone().unwrap_or_else(|| song.prompt.chars().take(60).collect());
    let mut out = Vec::new();
    for (i, (part, idx)) in wanted.iter().enumerate() {
        (ctx.progress)("Saving the parts", 0.8 + 0.2 * i as f64 / wanted.len() as f64);
        let len = parts[0].len();
        let [l, r] = [0, 1].map(|ch| (0..len).map(|s| idx.iter().map(|k| parts[*k].ch[ch][s]).sum::<f32>()).collect::<Vec<f32>>());
        let item = MediaItem {
            kind: "audio".into(),
            op: "stem".into(),
            prompt: song.prompt.clone(),
            title: Some(format!("{name} ({part})")),
            mime: "audio/wav".into(),
            seconds: len as f64 / stems::RATE as f64,
            parent: Some(song.id.clone()),
            chat_id: ctx.req.chat_id.clone().or_else(|| song.chat_id.clone()),
            lyrics: (*part == "vocals").then(|| song.lyrics.clone()).flatten(),
            ..Default::default()
        };
        out.push(store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &own_voice::wav_stereo(&l, &r, stems::RATE), None)?);
    }
    crate::applog::line("media", &format!("Took a song apart into {} parts", out.len()));
    Ok(out)
}

/// Remove FX (Suno Studio's): a sound's reverb and echo taken out
/// (`dereverb`), as a new sound kept out of the gallery: the Studio puts it
/// in as a take of the clip it came from.
async fn dry_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.as_deref().ok_or("Pick a sound first.")?;
    let song = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That sound is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Only a sound can have its effects removed.".into());
    }
    let helper = state.catalog.media.dereverb.clone().ok_or("Removing effects isn't available.")?;
    let card = map_card(ctx).await?;
    let (dll, model) = ensure_helper(state, &helper, "Getting the effects remover (once)", ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Removing the reverb and echo", 0.05);
    let bytes = store::read(&state.paths, &c, &song.id)?;
    let threads = state.limits().threads;
    let started = std::time::Instant::now();
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let (dry, on_card) = tokio::task::block_in_place(|| crate::cardfirst::asking(ask, || {
        let sound = decoded_stereo(&bytes, &song.mime)?;
        let done = dereverb::dry(&dll, &model, &sound, threads, card, ctx.cancel, &|f| (ctx.progress)("Removing the reverb and echo", 0.05 + 0.9 * f));
        dereverb::unload();
        done
    }))?;
    let name = song.title.clone().unwrap_or_else(|| song.prompt.chars().take(60).collect());
    let item = MediaItem {
        kind: "audio".into(),
        op: "dry".into(),
        prompt: song.prompt.clone(),
        title: Some(format!("{name} (dry)")),
        mime: "audio/wav".into(),
        seconds: dry.len() as f64 / dereverb::RATE as f64,
        parent: Some(song.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| song.chat_id.clone()),
        lyrics: song.lyrics.clone(),
        hidden: true,
        ..Default::default()
    };
    let saved = store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &own_voice::wav_stereo(&dry.ch[0], &dry.ch[1], dereverb::RATE), None)?;
    crate::applog::line("media", &format!("Removed the effects from {:.0} s of sound in {:.1} s ({})", saved.seconds, started.elapsed().as_secs_f64(), if on_card { "graphics card" } else { "processor" }));
    Ok(vec![saved])
}

/// A new part for the Studio (Suno Studio's generated stems): the mix so
/// far (`req.source`, rendered by the window) with the part added by
/// ACE-Step Base, then that part taken out alone; `count` takes (2 by
/// default, as Suno makes). Kept out of the gallery: they're the project's.
async fn studio_part(ctx: &RunCtx<'_>, model: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let source = req.source.clone().ok_or("Pick a song first.")?;
    let spec = req.music.clone().unwrap_or_default();
    let track = spec.track.clone().ok_or("Pick the part to make.")?;
    let takes = req.count.unwrap_or(2).clamp(1, 3);
    let seed = req.seed.unwrap_or_else(random_seed);
    let c = ctx.state.work_cipher()?;
    let mut out = Vec::new();
    for v in 0..takes {
        let share = |stage: u32, f: f64| (v as f64 + (stage as f64 + f) / 2.0) / takes as f64;
        let ask = |task: &str, audio: &str| Request {
            op: "music".into(),
            model_id: Some(model.id.clone()),
            prompt: req.prompt.clone(),
            lyrics: req.lyrics.clone(),
            seed: Some(seed + v as i64),
            language: req.language.clone(),
            music: Some(MusicSpec { task: Some(task.into()), audio: Some(audio.into()), track: Some(track.clone()), genre: spec.genre.clone(), mood: spec.mood.clone(), instruments: spec.instruments.clone(), ..Default::default() }),
            ..Default::default()
        };
        // The part added to the mix…
        let add = ask("add", &source);
        let p1 = |s: &str, f: f64| (ctx.progress)(&format!("Take {} of {takes}: {}", v + 1, s.to_lowercase()), share(0, f));
        let step = RunCtx { state: ctx.state, req: &add, cancel: ctx.cancel, progress: &p1, reserved: ctx.reserved };
        let dir = work.join(format!("part-{v}-add"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mixed = song(&step, model, &dir).await?.into_iter().next().ok_or("The music model didn't add the part.")?;
        // …then taken out alone.
        let take_out = ask("extract", &mixed.id);
        let p2 = |s: &str, f: f64| (ctx.progress)(&format!("Take {} of {takes}: {}", v + 1, s.to_lowercase()), share(1, f));
        let step = RunCtx { state: ctx.state, req: &take_out, cancel: ctx.cancel, progress: &p2, reserved: ctx.reserved };
        let dir = work.join(format!("part-{v}-alone"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let alone = song(&step, model, &dir).await;
        let _ = store::delete(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &mixed.id);
        let mut part = alone?.into_iter().next().ok_or("The music model didn't take the part out.")?;
        part.hidden = true;
        part.op = "studio_part".into();
        part.title = Some(format!("{} take {}", track.replace('_', " "), v + 1));
        store::update(&ctx.state.db.lock().unwrap(), &c, &part)?;
        out.push(part);
    }
    crate::applog::line("media", &format!("Made {} take(s) of a {} part for the Studio", out.len(), track));
    Ok(out)
}

#[tauri::command(async)]
pub fn studio_projects(state: AppStateRef) -> Result<Vec<serde_json::Value>, String> {
    let c = state.cipher()?;
    Ok(studio::list(&state.db.lock().unwrap(), &c))
}

#[tauri::command(async)]
pub fn studio_project_save(state: AppStateRef, project: serde_json::Value) -> Result<(), String> {
    let c = state.cipher()?;
    studio::save(&state.db.lock().unwrap(), &c, project)
}

#[tauri::command(async)]
pub fn studio_project_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    studio::remove(&state.db.lock().unwrap(), &c, &id)
}

#[tauri::command(async)]
pub fn studio_record_start(app: AppHandle, state: AppStateRef) -> Result<(), String> {
    crate::features::require(&state, Feature::Music)?;
    state.cipher()?;
    let mic = crate::speech::voice_settings(&state.db.lock().unwrap()).mic;
    studio::record_start(app, mic)
}

/// Stops recording and keeps it (out of the gallery: it's the project's).
#[tauri::command(async)]
pub fn studio_record_stop(state: AppStateRef) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let samples = studio::record_stop()?;
    let mut item = add(&state, &c, studio::recording_wav(&samples), "audio", "audio/wav", "recording", "Recording", None, None, true)?;
    item.seconds = samples.len() as f64 / 48_000.0;
    store::update(&state.db.lock().unwrap(), &c, &item)?;
    Ok(item)
}

/// A MIDI file the window wrote (a MIDI track's notes), saved where the
/// person picked in the save dialog: a .mid file only.
#[tauri::command(async)]
pub fn studio_save_midi(state: AppStateRef, path: String, data: String) -> Result<(), String> {
    state.cipher()?;
    let lower = path.to_lowercase();
    if !(lower.ends_with(".mid") || lower.ends_with(".midi")) {
        return Err("A MIDI file's name ends in .mid.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| "That MIDI couldn't be read.".to_string())?;
    if bytes.len() > 20_000_000 || !bytes.starts_with(b"MThd") {
        return Err("That isn't a MIDI file.".into());
    }
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save it there: {e}"))
}

/// A MIDI file the person picked in the open dialog, as base64.
#[tauri::command(async)]
pub fn studio_read_midi(state: AppStateRef, path: String) -> Result<String, String> {
    state.cipher()?;
    let lower = path.to_lowercase();
    if !(lower.ends_with(".mid") || lower.ends_with(".midi")) {
        return Err("Pick a MIDI file (.mid).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
    if bytes.len() > 20_000_000 || !bytes.starts_with(b"MThd") {
        return Err("That isn't a MIDI file.".into());
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// A sound file the person picked in the song studio, for the wavetable
/// synth to make a wavetable from (the window decodes it).
#[tauri::command(async)]
pub fn studio_read_sound(state: AppStateRef, path: String) -> Result<String, String> {
    state.cipher()?;
    let lower = path.to_lowercase();
    if ![".wav", ".mp3", ".flac", ".ogg", ".m4a", ".aac"].iter().any(|e| lower.ends_with(e)) {
        return Err("Pick a sound file (WAV, MP3, FLAC, OGG, M4A or AAC).".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
    if meta.len() > 50_000_000 {
        return Err("That file is too big for a wavetable (50 MB at most).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// What the song studio keeps by name: `kind` "synth" (the wavetable
/// synth's sounds) or "effects" (effects built from words or saved).
#[tauri::command(async)]
pub fn studio_library(state: AppStateRef, kind: String) -> Result<Vec<serde_json::Value>, String> {
    let c = state.cipher()?;
    Ok(studio::presets(&state.db.lock().unwrap(), &c, &kind))
}

#[tauri::command(async)]
pub fn studio_library_save(state: AppStateRef, kind: String, item: serde_json::Value) -> Result<(), String> {
    let c = state.cipher()?;
    studio::save_preset(&state.db.lock().unwrap(), &c, &kind, item)
}

#[tauri::command(async)]
pub fn studio_library_delete(state: AppStateRef, kind: String, name: String) -> Result<(), String> {
    let c = state.cipher()?;
    studio::remove_preset(&state.db.lock().unwrap(), &c, &kind, &name)
}

/// The song studio's chat bar: a request turned into actions on the
/// project (`studio::ask_messages`) by the chat model; the window does them.
#[tauri::command]
pub async fn studio_ask(state: AppStateRef<'_>, request: String, summary: String) -> Result<Vec<serde_json::Value>, String> {
    state.cipher()?;
    ask_studio(state.inner(), &request, &summary).await
}

pub(crate) async fn ask_studio(state: &Arc<AppState>, request: &str, summary: &str) -> Result<Vec<serde_json::Value>, String> {
    let (ep, _, _hold) = crate::background_endpoint(state).await.map_err(|_| "The chat bar needs a chat model. Install one from Models.".to_string())?;
    let answer = crate::chat::complete(&ep, studio::ask_messages(request.trim(), summary), json!({ "temperature": 0.2 }), 800).await?;
    let actions = studio::actions_of(&answer);
    if actions.is_empty() {
        return Err("The chat model didn't say what to do. Try saying it another way.".into());
    }
    Ok(actions)
}

#[tauri::command(async)]
pub fn studio_record_cancel() {
    studio::record_cancel();
}

/// A track's notes read from its sound (Suno Studio's audio to MIDI):
/// Basic Pitch's notes, or with `drums` the kick, snare and hat hits.
#[tauri::command]
pub async fn studio_midi(state: AppStateRef<'_>, id: String, drums: bool) -> Result<Vec<notes::Note>, String> {
    state.cipher()?;
    read_notes(state.inner(), &id, drums).await
}

#[cfg(test)]
pub(crate) async fn studio_midi_for_test(state: &Arc<AppState>, id: &str, drums: bool) -> Result<Vec<notes::Note>, String> {
    read_notes(state, id, drums).await
}

#[cfg(test)]
pub(crate) fn audio_in_decode_for_test(path: &std::path::Path) -> (Vec<f32>, u32, u16) {
    audio_in::decode(path).unwrap()
}

async fn read_notes(state: &Arc<AppState>, id: &str, drums: bool) -> Result<Vec<notes::Note>, String> {
    let c = state.work_cipher()?;
    let state = state.clone();
    let id = id.to_string();
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That sound is no longer here.")?;
    let bytes = store::read(&state.paths, &c, &id)?;
    let threads = state.limits().threads;
    if drums {
        return tokio::task::spawn_blocking(move || -> Result<Vec<notes::Note>, String> {
            let mix = decoded_stereo(&bytes, &item.mime)?;
            Ok(notes::drum_hits(&mix.mono(), stems::RATE))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let reader = state.catalog.media.note_reader.clone().ok_or("The note reader isn't available.")?;
    let never = AtomicBool::new(false);
    let (dll, model) = ensure_helper(&state, &reader, "Getting the note reader (once)", &never, &|_, _| {}).await?;
    tokio::task::spawn_blocking(move || -> Result<Vec<notes::Note>, String> {
        let mix = decoded_stereo(&bytes, &item.mime)?;
        let found = notes::melodic(&dll, &model, &stems::resample(&mix.mono(), stems::RATE, notes::RATE), threads, None);
        notes::unload();
        found
    })
    .await
    .map_err(|e| e.to_string())?
}

/// About how long a training job takes: labelling each song, turning it
/// into tensors, and its epochs (about 0.8 s a song an epoch on an RTX 5070
/// for songs of up to four minutes), plus setting the trainer up the first time.
fn train_estimate(state: &AppState, req: &Request) -> f64 {
    let n = req.train.as_ref().map_or(0, |t| t.files.len() + t.songs.len());
    let setup = match &state.catalog.media.trainer {
        Some(t) if !train::ready(&state.paths, t) => 900.0,
        _ => 60.0,
    };
    setup + n as f64 * 40.0 + train::epochs_for(n) as f64 * n as f64 * 0.8
}

/// The trainer, set up the first time (`train`): uv, ACE-Step's source and
/// its locked Python packages, and the Base model's training files.
#[cfg(windows)]
async fn ensure_trainer(ctx: &RunCtx<'_>, t: &train::TrainerCatalog) -> Result<PathBuf, String> {
    let state = ctx.state;
    let paths = &state.paths;
    (ctx.progress)("Getting the trainer (once)", 0.01);
    let uv = engine::ensure_unpacked(paths, "trainer-uv-bin", &t.uv, "uv.exe", ctx.cancel, |r, all| (ctx.progress)("Getting the trainer (once)", 0.01 * r as f64 / all.max(1) as f64)).await?;
    let source = train::source_dir(paths, t);
    let dir_name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
    engine::ensure_unpacked(paths, &dir_name, &t.source, "train.py", ctx.cancel, |r, all| (ctx.progress)("Getting the trainer (once)", 0.01 + 0.01 * r as f64 / all.max(1) as f64)).await?;
    if !source.join(".venv").join("Scripts").join("python.exe").is_file() {
        // Its Python and packages (PyTorch for NVIDIA cards and the rest), as locked.
        state.log("network", "Downloading the music trainer's Python packages from pypi.org and download.pytorch.org");
        (ctx.progress)("Setting up the trainer (once, about 6 GB)", 0.02);
        let ran = proc::run_env(
            proc::Run {
                exe: &uv,
                args: ["sync", "--frozen", "--python", "3.12"].map(std::ffi::OsString::from).to_vec(),
                cwd: &source,
                log: &paths.logs.join("music-trainer-setup.log"),
                low_priority: state.limits().low_priority,
                job: state.job(),
            },
            &train::trainer_env(paths),
            ctx.cancel,
            |_| {},
        )
        .await;
        if let Err(e) = ran {
            // A half-made environment would look ready next time.
            let _ = std::fs::remove_dir_all(source.join(".venv"));
            return Err(e);
        }
    }
    let ckpt = train::checkpoints_dir(paths);
    let missing: Vec<&train::TrainerFile> = t.checkpoints.iter().filter(|f| !ckpt.join(&f.file).is_file()).collect();
    if !missing.is_empty() {
        state.log("network", "Downloading ACE-Step 1.5 Base's training files from huggingface.co");
        let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false)?;
        let total: u64 = missing.iter().map(|f| f.size).sum::<u64>().max(1);
        let mut done = 0u64;
        for f in missing {
            let base = done;
            download::fetch_verified(&client, &f.url, &ckpt.join(&f.file), f.size, &f.sha256, ctx.cancel, |r, _| {
                (ctx.progress)("Getting the Base model's training files (once)", 0.03 + 0.05 * (base + r) as f64 / total as f64)
            })
            .await?;
            done += f.size;
        }
    }
    Ok(uv)
}

/// A custom model trained on the person's songs (`train`): each song's
/// words heard, the songs turned into the trainer's tensors, then trained;
/// the adapter is kept with the music models and the model listed.
#[cfg(windows)]
async fn train_job(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let state = ctx.state;
    let spec = ctx.req.train.clone().ok_or("Pick your songs first.")?;
    let t = state.catalog.media.trainer.clone().ok_or("Training isn't available on this system yet.")?;
    let started = Instant::now();
    let uv = ensure_trainer(ctx, &t).await?;
    let c = state.work_cipher()?;
    // The songs, copied for the trainer (it reads MP3, WAV, FLAC, OGG, Opus, M4A).
    let audio = work.join("audio");
    std::fs::create_dir_all(&audio).map_err(|e| e.to_string())?;
    let mut copies: Vec<PathBuf> = Vec::new();
    for f in &spec.files {
        let from = PathBuf::from(f);
        let ext = from.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a") {
            return Err(format!("{} isn't a song file the trainer reads (MP3, WAV, FLAC, OGG, Opus or M4A).", from.file_name().unwrap_or_default().to_string_lossy()));
        }
        let to = audio.join(format!("{:02}.{ext}", copies.len() + 1));
        std::fs::copy(&from, &to).map_err(|e| format!("Couldn't read {}: {e}", from.display()))?;
        copies.push(to);
    }
    // A recorded voice's singing, take by take (short ones teach little).
    if let Some(vid) = spec.voice.as_deref().filter(|v| !v.is_empty()) {
        let voice = own_voice::get(&state.db.lock().unwrap(), &c, vid).ok_or("That recorded voice is no longer here.")?;
        let takes: Vec<Vec<f32>> = own_voice::sung_takes(&state.paths, &c, &voice)?.into_iter().filter(|t| t.len() >= 48_000 * 4).collect();
        if takes.is_empty() {
            return Err(format!("“{}” has no singing recorded yet (4 seconds or more): sing the singing lines in Your voice, or add songs where you sing.", voice.name));
        }
        for take in takes {
            let to = audio.join(format!("{:02}.wav", copies.len() + 1));
            std::fs::write(&to, crate::audio::wav_encode(&take, 48_000)).map_err(|e| e.to_string())?;
            copies.push(to);
        }
    }
    for id in &spec.songs {
        let item = store::get(&state.db.lock().unwrap(), &c, id).ok_or("A song picked is no longer in the gallery.")?;
        let ext = if item.mime.contains("wav") { "wav" } else { "mp3" };
        let to = audio.join(format!("{:02}.{ext}", copies.len() + 1));
        std::fs::write(&to, store::read(&state.paths, &c, id)?).map_err(|e| e.to_string())?;
        copies.push(to);
    }
    // Each song's words, heard by speech recognition (none installed: all
    // are taught as they sound, without words).
    let listen = !crate::speech::installed(&state.db.lock().unwrap()).is_empty();
    let mut samples = Vec::new();
    for (i, file) in copies.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        (ctx.progress)(&format!("Listening to song {} of {}", i + 1, copies.len()), 0.08 + 0.1 * i as f64 / copies.len() as f64);
        let path = file.clone();
        let (mono, seconds) = tokio::task::spawn_blocking(move || -> Result<(Vec<f32>, f64), String> {
            let (samples, rate, channels) = audio_in::decode(&path)?;
            let ch = channels.max(1) as usize;
            let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
            let seconds = mono.len() as f64 / rate as f64;
            Ok((stems::resample(&mono, rate, crate::audio::RATE), seconds))
        })
        .await
        .map_err(|e| e.to_string())??;
        if seconds < 10.0 {
            return Err(format!("Song {} is only {seconds:.0} seconds: use whole songs (30 seconds to 4 minutes).", i + 1));
        }
        let lyrics = if listen {
            let heard = crate::speech::transcribe_song(state, &mono, None).await?;
            train::lyrics_from(&heard.into_iter().map(|s| s.text).collect::<Vec<_>>())
        } else {
            None
        };
        samples.push(train::Sample { file: file.clone(), lyrics, seconds });
    }
    let trigger = train::trigger_of(&spec.name);
    let description = Some(spec.description.trim()).filter(|d| !d.is_empty()).unwrap_or("song").to_string();
    std::fs::write(work.join("dataset.json"), train::dataset_json(&samples, &description, &trigger).to_string()).map_err(|e| e.to_string())?;
    let n = samples.len();
    // (Tests train briefly.)
    let short = cfg!(test).then(|| std::env::var("SULCUSAI_TRAIN_EPOCHS").ok()).flatten().and_then(|e| e.parse().ok());
    let epochs = short.unwrap_or_else(|| train::epochs_for(n));
    let source = train::source_dir(&state.paths, &t);
    let ckpt = train::checkpoints_dir(&state.paths);
    let env = train::trainer_env(&state.paths);
    let log = state.paths.logs.join("music-trainer.log");
    let low = state.limits().low_priority;
    // Turned into tensors (two passes over the songs), then trained.
    let (mut pass1, mut pass2) = (0u32, 0u32);
    proc::run_env(
        proc::Run { exe: &uv, args: train::trainer_args(&source, &ckpt, work, epochs, true), cwd: work, log: &log, low_priority: low, job: state.job() },
        &env,
        ctx.cancel,
        |l| match train::progress_of(l, n) {
            Some(("pass1", ..)) => {
                pass1 += 1;
                (ctx.progress)("Reading your songs", 0.18 + 0.06 * pass1 as f64 / n as f64)
            }
            Some(("pass2", ..)) => {
                pass2 += 1;
                (ctx.progress)("Reading your songs", 0.24 + 0.06 * pass2 as f64 / n as f64)
            }
            _ => {}
        },
    )
    .await?;
    proc::run_env(
        proc::Run { exe: &uv, args: train::trainer_args(&source, &ckpt, work, epochs, false), cwd: work, log: &log, low_priority: low, job: state.job() },
        &env,
        ctx.cancel,
        |l| {
            if let Some(("epoch", done, of)) = train::progress_of(l, n) {
                (ctx.progress)(&format!("Learning your songs: pass {done} of {of}"), 0.3 + 0.68 * done as f64 / of.max(1) as f64);
            }
        },
    )
    .await?;
    let made = work.join("out").join("final");
    let id = store::new_id();
    let dest = train::adapters_dir(&state.paths).join(&id);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for f in ["adapter_model.safetensors", "adapter_config.json"] {
        std::fs::copy(made.join(f), dest.join(f)).map_err(|_| "The trainer didn't write the model. See music-trainer.log in Settings › Logs.".to_string())?;
    }
    let minutes = started.elapsed().as_secs_f64() / 60.0;
    let model = train::CustomModel { id, name: spec.name.trim().chars().take(60).collect(), trigger, description, songs: n, epochs, minutes, created_at: db::now_ms() };
    train::add(&state.db.lock().unwrap(), &c, model.clone())?;
    crate::applog::line("media", &format!("Trained a custom model on {n} songs ({epochs} passes) in {minutes:.0} min"));
    emit(state, "media:trained", json!(model));
    Ok(Vec::new())
}

#[tauri::command(async)]
pub fn music_custom_models(state: AppStateRef) -> Result<serde_json::Value, String> {
    let c = state.cipher()?;
    let models = train::list(&state.db.lock().unwrap(), &c);
    let t = state.catalog.media.trainer.as_ref();
    Ok(json!({
        "models": models,
        "available": t.is_some() && cfg!(windows),
        // Recorded voices and how much singing each has, to train on.
        "voices": own_voice::list(&state.db.lock().unwrap(), &c).iter().map(|v| json!({ "id": v.id, "name": v.name, "sung": own_voice::sung_count(v) })).collect::<Vec<_>>(),
        "nvidia": crate::natural::nvidia_here(&state),
        "ready": t.is_some_and(|t| train::ready(&state.paths, t)),
        "setup_bytes": t.map_or(0, train::setup_bytes),
        "base_installed": installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == train::BASE_MODEL),
    }))
}

/// The ways a picture can be made to talk here: quick (Ditto, NVIDIA
/// cards) and detailed (Wan 2.2 S2V, when installed).
#[tauri::command(async)]
pub fn media_talk_ways(state: AppStateRef) -> serde_json::Value {
    let d = state.catalog.media.ditto.as_ref();
    let detailed = installed(&state.db.lock().unwrap()).iter().any(|i| state.catalog.media.models.iter().any(|m| m.id == i.model_id && m.kind == Kind::Video && m.can("speech")));
    json!({
        "quick": d.is_some() && cfg!(windows) && crate::natural::nvidia_here(&state),
        "ready": d.is_some_and(|d| ditto::ready(&state.paths, d)),
        "setup_bytes": d.map_or(0, ditto::setup_bytes),
        "detailed": detailed,
    })
}

#[tauri::command(async)]
pub fn music_custom_model_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    train::remove(&state.db.lock().unwrap(), &c, &state.paths, &id)
}

/// A song's video (Suno's Create video, `songvideo`): its cover art, title
/// and lyrics as they're sung, with the song as the sound, kept in the
/// gallery under the song. Where the lines are sung comes from the
/// listener, who listens first when the song hasn't been listened to.
#[cfg(windows)]
async fn song_video_job(ctx: &RunCtx<'_>, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    use songvideo::{FPS, SIDE};
    let state = ctx.state;
    let c = state.work_cipher()?;
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let mut song = store::get(&state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Only a song can be made into a video.".into());
    }
    let sung = song.lyrics.as_deref().is_some_and(lyrics::has_singing);
    if sung && song.review.is_none() && !crate::speech::installed(&state.db.lock().unwrap()).is_empty() {
        (ctx.progress)("Listening for where the words are", 0.05);
        song.review = Some(hear(state, &song, ctx.cancel).await?);
        store::update(&state.db.lock().unwrap(), &c, &song)?;
    }
    let art = if song.poster { store::read_thumb(&state.paths, &c, &song.id).ok().and_then(|b| image::load_from_memory(&b).ok()).map(|i| i.to_rgba8()) } else { None };
    let bytes = store::read(&state.paths, &c, &song.id)?;
    let out = work.join("song.mp4");
    let name = song.title.clone().unwrap_or_else(|| song.prompt.chars().take(80).collect());
    let lines: Vec<(String, Option<[f64; 2]>)> = match (&song.review, song.lyrics.as_deref()) {
        (Some(r), _) if !r.lines.is_empty() => r.lines.iter().map(|l| (l.text.clone(), l.at)).collect(),
        (_, Some(l)) if sung => l.lines().map(str::trim).filter(|x| !x.is_empty() && !(x.starts_with('[') && x.ends_with(']'))).map(|x| (x.to_string(), None)).collect(),
        _ => Vec::new(),
    };
    let first = tokio::task::block_in_place(|| -> Result<image::RgbaImage, String> {
        // The sound at 48 kHz (or 44.1), 16-bit stereo.
        let mix = decoded_stereo(&bytes, &song.mime)?;
        let rate = if mix.rate == 44_100 { 44_100 } else { 48_000 };
        let mix = mix.resampled(rate);
        let seconds = mix.len() as f64 / rate as f64;
        let pcm: Vec<i16> = (0..mix.len()).flat_map(|i| [mix.ch[0][i], mix.ch[1][i]]).map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        let timed = songvideo::timings(&lines, seconds);
        let title = songvideo::text_mask(&name, 54, true, SIDE - 120)?;
        let base = songvideo::backdrop(art.as_ref(), &name, Some(&title));
        let lyric_y = if art.is_some() { 690 + title.height() + 24 } else { 300 + title.height() + 80 };
        let mut masks: std::collections::HashMap<(usize, bool), image::GrayImage> = std::collections::HashMap::new();
        // The picture changes a line at a time: 1.5 Mbps is plenty.
        let mut writer = mp4::Writer::create_with_sound(&out, SIDE, SIDE, FPS, Some(rate), Some(1_500_000))?;
        let frames = (seconds * FPS as f64).ceil() as usize;
        let mut first = None;
        let mut sent = 0usize;
        for f in 0..frames {
            if f % FPS as usize == 0 {
                if ctx.cancel.load(Ordering::Relaxed) {
                    return Err(download::CANCELLED.into());
                }
                (ctx.progress)("Making the video", 0.1 + 0.85 * f as f64 / frames as f64);
                // The sound a second ahead of the pictures.
                let upto = (((f / FPS as usize) + 1) * rate as usize * 2).min(pcm.len());
                writer.sound(&pcm[sent..upto])?;
                sent = upto;
            }
            let t = f as f64 / FPS as f64;
            let mut frame = base.clone();
            let (now, next) = songvideo::showing(&timed, t);
            let mut y = lyric_y;
            for (i, big) in [(now, true), (next, false)].into_iter().filter_map(|(i, b)| i.map(|i| (i, b))) {
                if !masks.contains_key(&(i, big)) {
                    masks.insert((i, big), songvideo::text_mask(&timed[i].text, if big { 44 } else { 34 }, big, SIDE - 140)?);
                }
                let m = &masks[&(i, big)];
                songvideo::stamp(&mut frame, m, (SIDE - m.width()) / 2, y, [255, 255, 255], if big { 1.0 } else { 0.5 });
                y += m.height() + 10;
            }
            // How far along: a thin bar at the bottom.
            let done = ((t / seconds.max(0.1)) * SIDE as f64) as u32;
            for x in 0..done.min(SIDE) {
                for yy in SIDE - 10..SIDE - 4 {
                    frame.put_pixel(x, yy, image::Rgba([255, 255, 255, 255]));
                }
            }
            if first.is_none() && (now.is_some() || f as f64 >= seconds * 0.2) {
                first = Some(frame.clone());
            }
            writer.frame(&frame)?;
        }
        writer.sound(&pcm[sent..])?;
        writer.finish()?;
        Ok(first.unwrap_or(base))
    })?;
    let mp4 = std::fs::read(&out).map_err(|e| e.to_string())?;
    let thumb = imaging::thumbnail(&image::DynamicImage::ImageRgba8(first))?;
    let item = MediaItem {
        kind: "video".into(),
        op: "song_video".into(),
        prompt: song.prompt.clone(),
        title: song.title.clone(),
        mime: "video/mp4".into(),
        width: SIDE,
        height: SIDE,
        seconds: song.seconds,
        parent: Some(song.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| song.chat_id.clone()),
        lyrics: song.lyrics.clone(),
        poster: true,
        ..Default::default()
    };
    crate::applog::line("media", &format!("Made a song video: {:.0} s, {} lines", song.seconds, lines.len()));
    Ok(vec![store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &mp4, Some(&thumb))?])
}

/// What the listener hears in a song from the gallery (`listen`): which of
/// its words come through and where, from speech recognition. Nothing heard
/// when no speech model is installed (the review then says so).
async fn hear(state: &Arc<AppState>, item: &MediaItem, cancel: &AtomicBool) -> Result<listen::Review, String> {
    let c = state.work_cipher()?;
    let bytes = store::read(&state.paths, &c, &item.id)?;
    let lyrics = item.lyrics.clone().unwrap_or_default();
    let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>, String> {
        let dir = std::env::temp_dir().join(format!("sulcusai-listen-{}", store::new_id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("song.mp3");
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        let decoded = audio_in::decode(&path);
        let _ = std::fs::remove_dir_all(&dir);
        let (samples, rate, channels) = decoded?;
        let ch = channels.max(1) as usize;
        let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
        Ok(stems::resample(&mono, rate, crate::audio::RATE))
    })
    .await
    .map_err(|e| e.to_string())??;
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    // No speech model installed: the review says so.
    let installed = !crate::speech::installed(&state.db.lock().unwrap()).is_empty();
    let heard = if installed { Some(crate::speech::transcribe_song(state, &samples, None).await?) } else { None };
    Ok(listen::review(&lyrics, heard.as_deref(), item.seconds))
}

/// Of several versions of a sung song, the one whose words were heard best
/// (the first of equals), with its review; the others are deleted. All are
/// kept, as made, when there are no words or nothing to listen with.
async fn keep_clearest(ctx: &RunCtx<'_>, items: Vec<MediaItem>) -> Result<Vec<MediaItem>, String> {
    if !items.iter().all(|i| i.lyrics.as_deref().is_some_and(lyrics::has_singing)) || crate::speech::installed(&ctx.state.db.lock().unwrap()).is_empty() {
        crate::applog::line("media", "Kept every version: nothing sung, or no speech model to listen with");
        return Ok(items);
    }
    let c = ctx.state.work_cipher()?;
    let n = items.len();
    let mut heard = Vec::new();
    for (i, item) in items.iter().enumerate() {
        (ctx.progress)(&format!("Listening to version {} of {n}", i + 1), i as f64 / n as f64);
        heard.push(hear(ctx.state, item, ctx.cancel).await?);
    }
    let best = (0..n).fold(0, |b, i| if heard[i].score > heard[b].score { i } else { b });
    crate::applog::line("media", &format!("Kept version {} of {n}: scores {:?}", best + 1, heard.iter().map(|r| r.score).collect::<Vec<_>>()));
    let mut kept = None;
    for (i, (mut item, review)) in items.into_iter().zip(heard).enumerate() {
        if i == best {
            let clearest = format!("The clearest of {n} versions: {} of {} words heard.", review.heard, review.words);
            item.note = Some(item.note.take().map_or(clearest.clone(), |was| format!("{was} {clearest}")));
            item.review = Some(review);
            store::update(&ctx.state.db.lock().unwrap(), &c, &item)?;
            kept = Some(item);
        } else {
            store::delete(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &item.id)?;
        }
    }
    Ok(kept.into_iter().collect())
}

/// Listens to a song (`req.source`) and keeps the review on it.
async fn listen_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song to listen to.")?;
    let c = ctx.state.work_cipher()?;
    let mut item = store::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if item.kind != "audio" || item.lyrics.as_deref().is_none_or(|l| !lyrics::has_singing(l)) {
        return Err("Only a sung song can be listened to.".into());
    }
    (ctx.progress)("Listening", 0.2);
    item.review = Some(hear(ctx.state, &item, ctx.cancel).await?);
    store::update(&ctx.state.db.lock().unwrap(), &c, &item)?;
    crate::applog::line("media", &format!("Listened to a song: score {}", item.review.as_ref().map_or(0, |r| r.score)));
    Ok(vec![item])
}

/// How a song (`req.source`) is built, for the song view: its parts taken
/// apart (HTDemucs), their notes read (Basic Pitch; drums by their hits),
/// and its sections placed where their words are sung (the listener's
/// review, listened for first when the song has none). Kept on the song.
async fn map_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let item = store::get(&ctx.state.db.lock().unwrap(), &ctx.state.work_cipher()?, id).ok_or("That song is no longer in the gallery.")?;
    Ok(vec![map_item(ctx, item).await?])
}

/// The graphics card a song's work (its parts, its voice sung again) runs
/// on, the cards' ONNX Runtime downloaded the first time. When no card can
/// take it, why is said and the person asked before the processor does it
/// (`cardfirst`); None then.
async fn map_card(ctx: &RunCtx<'_>) -> Result<Option<crate::natural::Card>, String> {
    let state = ctx.state;
    // Started now on the processor, as they asked.
    if ON_PROCESSOR.try_with(|p| *p).unwrap_or(false) {
        (ctx.progress)("On the processor, as you asked (slower)", 0.0);
        return Ok(None);
    }
    loop {
        let why = match card_for_onnx(ctx).await {
            // The rest runs on the card through DirectML; the part separator
            // can't (`stems::separate`), so it's the processor's, once they say yes.
            Ok(card) if card.dml => {
                let job = JOB_ID.try_with(|j| j.clone()).ok();
                let why = "this graphics card works here through DirectML, which can't run the part separator (the rest of the work stays on the card)";
                crate::cardfirst::processor_ok(state, job.as_deref(), "Taking the song apart", why, ctx.cancel, ctx.progress).await?;
                return Ok(Some(card));
            }
            Ok(card) => return Ok(Some(card)),
            Err(e) if e == download::CANCELLED => return Err(e),
            Err(why) => why,
        };
        let job = JOB_ID.try_with(|j| j.clone()).ok();
        match crate::cardfirst::processor_ok(state, job.as_deref(), "This song's work", &why, ctx.cancel, ctx.progress).await? {
            crate::cardfirst::Go::Again => continue,
            crate::cardfirst::Go::Processor => {
                (ctx.progress)(PROCESSOR_STAGE, 0.0);
                return Ok(None);
            }
        }
    }
}

/// The card for ONNX Runtime's work now (`natural::pick_card`), or why not.
async fn card_for_onnx(ctx: &RunCtx<'_>) -> Result<crate::natural::Card, String> {
    let state = ctx.state;
    let voices = &state.catalog.voices;
    if !crate::natural::card_here(state) {
        return Err(if state.hardware.read().unwrap().best_gpu().is_some() { "there's no ONNX Runtime for graphics cards on this system".into() } else { crate::natural::no_card(state) });
    }
    if crate::natural::card_runtime_dll(&state.paths, voices).is_none() {
        let getting = format!("Getting the graphics card's libraries (once, {})", crate::size_label(crate::natural::card_runtime_size(&state.paths, voices, false)));
        (ctx.progress)(&getting, 0.0);
        match crate::natural::ensure_card_runtime(state, false, ctx.cancel, &|r, t| (ctx.progress)(&getting, r as f64 / t.max(1) as f64)).await {
            Ok(_) => {}
            Err(e) if e == download::CANCELLED => return Err(e),
            Err(e) => return Err(format!("the graphics card's libraries couldn't be downloaded ({e})")),
        }
    }
    if crate::natural::loaded_for_processor() {
        return Err("SulcusAI loaded the processor's libraries before the graphics card's were here, and only one can be loaded until it starts again".into());
    }
    // The second card: the job's place in line, or handed over to it.
    let job = JOB_ID.try_with(|j| j.clone()).ok();
    let second = SECOND_CARD.try_with(|s| *s).unwrap_or(false) || job.as_deref().and_then(crate::lineup::card_of) == Some(crate::lineup::Card::Second);
    let mut p = crate::natural::pick_card(state, second).await?;
    // CUDA's part, the first time an NVIDIA card it runs on takes work.
    if p.needs_cuda {
        let getting = format!("Getting the graphics card's libraries (once, {})", crate::size_label(crate::natural::card_runtime_size(&state.paths, voices, true)));
        (ctx.progress)(&getting, 0.0);
        match crate::natural::ensure_card_runtime(state, true, ctx.cancel, &|r, t| (ctx.progress)(&getting, r as f64 / t.max(1) as f64)).await {
            Ok(_) => {}
            Err(e) if e == download::CANCELLED => return Err(e),
            // DirectML on the same card meanwhile.
            Err(e) => match state.hardware.read().unwrap().gpus.iter().find(|g| g.name == p.name).and_then(|g| g.dxgi) {
                Some(i) => {
                    crate::applog::line("media", &format!("Couldn't get CUDA's part ({e}); DirectML runs the work on {} instead", p.name));
                    p.card = crate::natural::Card { device: i as i32, dml: true };
                }
                None => return Err(format!("the graphics card's libraries couldn't be downloaded ({e})")),
            },
        }
    }
    crate::applog::line("media", &format!("On {}{} ({})", p.name, if p.second { ", the second card" } else { "" }, if p.card.dml { "DirectML" } else { "CUDA" }));
    Ok(p.card)
}

/// `map_job` for one song, which is kept with its map.
async fn map_item(ctx: &RunCtx<'_>, mut item: MediaItem) -> Result<MediaItem, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    if item.kind != "audio" {
        return Err("Only a song can be shown this way.".into());
    }
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    let reader = state.catalog.media.note_reader.clone().ok_or("The note reader isn't available.")?;
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    let (_, note_reader) = ensure_helper(state, &reader, "Getting the note reader (once)", ctx.cancel, ctx.progress).await?;
    if item.review.is_none() && item.lyrics.as_deref().is_some_and(lyrics::has_singing) {
        (ctx.progress)("Listening for where the words are", 0.1);
        item.review = Some(hear(state, &item, ctx.cancel).await?);
    }
    (ctx.progress)("Taking the parts apart", 0.4);
    let bytes = store::read(&state.paths, &c, &item.id)?;
    let threads = state.limits().threads;
    let mime = item.mime.clone();
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let (parts, on_card, times) = tokio::task::spawn_blocking(move || crate::cardfirst::asking(ask, || -> Result<(Vec<songmap::Part>, bool, [f64; 2]), String> {
        let song = decoded_stereo(&bytes, &mime)?;
        let started = Instant::now();
        let (stems, on_card) = stems::separate(&dll, &separator, &song, threads, &[0, 1, 2, 3], card, &AtomicBool::new(false))?;
        stems::unload();
        let apart = started.elapsed().as_secs_f64();
        let card = card.filter(|_| on_card);
        let mut parts = Vec::new();
        // The voice first, then the other instruments, the bass and the drums.
        for i in [3usize, 2, 1, 0] {
            let mono = stems[i].mono();
            let notes = if i == 0 {
                notes::drum_hits(&mono, stems::RATE)
            } else {
                let mut n = notes::melodic(&dll, &note_reader, &stems::resample(&mono, stems::RATE, notes::RATE), threads, card)?;
                // A bass part's notes are low; what's higher is another instrument's spill.
                if i == 1 {
                    n.retain(|x| x.pitch < 60);
                }
                n
            };
            parts.push(songmap::Part { name: stems::PARTS[i].to_string(), notes });
        }
        notes::unload();
        Ok((parts, on_card, [apart, started.elapsed().as_secs_f64() - apart]))
    }))
    .await
    .map_err(|e| e.to_string())??;
    crate::applog::line(
        "media",
        &format!("Took a song's parts apart in {:.1}s and read their notes in {:.1}s on the {}", times[0], times[1], if on_card { "graphics card" } else { "processor" }),
    );
    if card.is_some() && !on_card {
        (ctx.progress)(PROCESSOR_STAGE, 0.9);
    }
    let map = songmap::map(item.review.as_ref(), parts, item.seconds);
    crate::applog::line(
        "media",
        &format!("Mapped a song: {} sections, {} notes", map.sections.len(), map.parts.iter().map(|p| p.notes.len()).sum::<usize>()),
    );
    item.map = Some(map);
    store::update(&state.db.lock().unwrap(), &c, &item)?;
    Ok(item)
}

/// Sings a song from the gallery (`req.source`) again in a recorded voice
/// (`req.voice`).
async fn revoice_job(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let voice = ctx.req.voice.clone().ok_or("Pick whose voice sings it.")?;
    let item = store::get(&ctx.state.db.lock().unwrap(), &ctx.state.work_cipher()?, id).ok_or("That song is no longer in the gallery.")?;
    if item.kind != "audio" {
        return Err("Only a song can be sung in another voice.".into());
    }
    Ok(vec![revoice_item(ctx, &item, &voice).await?])
}

/// A song with its voice sung again in a recorded one: the voice taken out
/// (HTDemucs), sung in theirs (`svc`, whole octaves up or down to suit
/// them), and put back over the band at its old loudness. Kept as a new
/// song beside the first.
async fn revoice_item(ctx: &RunCtx<'_>, item: &MediaItem, voice_id: &str) -> Result<MediaItem, String> {
    let state = ctx.state;
    let c = state.work_cipher()?;
    let voice = own_voice::get(&state.db.lock().unwrap(), &c, voice_id).ok_or("That recorded voice is no longer here.")?;
    let (reference, sung) = own_voice::singing_reference(&state.paths, &c, &voice)?;
    let sep = state.catalog.media.separator.clone().ok_or("The part separator isn't available.")?;
    // With a second card and something waiting for the main one: sung there,
    // and the main card goes on to the next in line.
    if let Some(job) = JOB_ID.try_with(|j| j.clone()).ok() {
        crate::lineup::hand_over(state, &job);
    }
    let card = map_card(ctx).await?;
    let (dll, separator) = ensure_helper(state, &sep, "Getting the part separator (once)", ctx.cancel, ctx.progress).await?;
    let models = own_voice::sing_parts(state, ctx.cancel, ctx.progress).await?;
    (ctx.progress)("Taking the voice out", 0.05);
    let bytes = store::read(&state.paths, &c, &item.id)?;
    let threads = state.limits().threads;
    let singing = format!("Singing it in the recorded voice “{}”", voice.name);
    // Long work that reports as it goes and can be stopped: it borrows the
    // job's progress and cancel, so it runs here rather than on its own thread.
    let ask = crate::cardfirst::hook_for_job(state, JOB_ID.try_with(|j| j.clone()).ok());
    let (out, on_card, secs) = tokio::task::block_in_place(|| crate::cardfirst::asking(ask, || -> Result<(stems::Stereo, (bool, bool), f64), String> {
        let started = Instant::now();
        let song = decoded_stereo(&bytes, &item.mime)?;
        let (parts, apart_on_card) = stems::separate(&dll, &separator, &song, threads, &[0, 1, 2, 3], card, ctx.cancel)?;
        stems::unload();
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        let band = stems::Stereo {
            rate: stems::RATE,
            ch: [0, 1].map(|c| (0..song.len()).map(|i| parts[0].ch[c][i] + parts[1].ch[c][i] + parts[2].ch[c][i]).collect()),
        };
        let sung_part = parts[3].mono();
        drop(parts);
        (ctx.progress)(&singing, 0.25);
        let mut conv = svc::Converter::new(&dll, &models, threads, card)?;
        let v = conv.voice(&reference)?;
        // Speech sits about four semitones under the same person's singing.
        let person = if sung { v.pitch } else { v.pitch * 1.26 };
        let semitones = svc::octave_shift(conv.middle_pitch_of(&sung_part)?, person);
        // Seed-VC's 30 steps on a card; on the processor each is ~6 s for
        // 30 s of song, so fewer.
        let steps = if conv.on_card { 30 } else { 10 };
        let new_voice = conv.convert(&v, &sung_part, svc::Settings { steps, semitones, ..Default::default() }, ctx.cancel, &|f| (ctx.progress)(&singing, 0.25 + 0.7 * f))?;
        let on_card = (apart_on_card, conv.on_card);
        drop(conv);
        crate::applog::line("media", &format!("Sang a song again in a recorded voice: {semitones} semitones, {steps} steps"));
        Ok((own_voice::mix(&band, &sung_part, &new_voice), on_card, started.elapsed().as_secs_f64()))
    }))?;
    let on = |card: bool| if card { "graphics card" } else { "processor" };
    crate::applog::line("media", &format!("Sang a {:.0} s song in a recorded voice in {secs:.0} s: taken apart on the {}, sung on the {}", out.len() as f64 / out.rate as f64, on(on_card.0), on(on_card.1)));
    let wav = own_voice::wav_stereo(&out.ch[0], &out.ch[1], out.rate);
    let new = MediaItem {
        kind: "audio".into(),
        op: item.op.clone(),
        prompt: item.prompt.clone(),
        model_id: item.model_id.clone(),
        mime: "audio/wav".into(),
        seconds: out.len() as f64 / out.rate as f64,
        parent: Some(item.id.clone()),
        chat_id: ctx.req.chat_id.clone().or_else(|| item.chat_id.clone()),
        seed: item.seed,
        title: item.title.clone(),
        music: item.music.clone(),
        lyrics: item.lyrics.clone(),
        note: Some(format!("Sung in the recorded voice “{}”", voice.name)),
        ..Default::default()
    };
    store::save(&state.db.lock().unwrap(), &state.paths, &c, new, &wav, None)
}

/// Makes again the stretches of a song (`req.source`) whose words weren't
/// clear, one after another on the same song (the music model's repaint,
/// asked for a clear voice), then listens to the result. The steps between
/// are kept out of the gallery; the song it started from stays.
async fn remake_job(ctx: &RunCtx<'_>, model: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.clone().ok_or("Pick a song to make again.")?;
    let c = ctx.state.work_cipher()?;
    let original = store::get(&ctx.state.db.lock().unwrap(), &c, &id).ok_or("That song is no longer in the gallery.")?;
    let review = original.review.clone().ok_or("Listen to the song first.")?;
    if review.unclear.is_empty() {
        return Err("Every part of this song was already clear.".into());
    }
    let mut current = original.clone();
    let spans: Vec<[f64; 2]> = review.unclear.iter().take(4).copied().collect();
    for (i, span) in spans.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        let req = Request {
            op: "music".into(),
            model_id: Some(model.id.clone()),
            prompt: format!("{}, clear lead vocals upfront, crisp diction", original.prompt.trim().trim_end_matches('.')),
            lyrics: original.lyrics.clone(),
            seed: original.seed.map(|s| s + 1 + i as i64),
            music: Some(MusicSpec { task: Some("repaint".into()), audio: Some(current.id.clone()), region: Some([span[0] as f32, span[1] as f32]), ..Default::default() }),
            chat_id: original.chat_id.clone(),
            ..Default::default()
        };
        let label = format!("Making {} to {} again", clock(span[0]), clock(span[1]));
        let progress = |stage: &str, f: f64| (ctx.progress)(&format!("{label}: {}", stage.to_lowercase()), (i as f64 + f) / (spans.len() as f64 + 1.0));
        let step = RunCtx { state: ctx.state, req: &req, cancel: ctx.cancel, progress: &progress, reserved: ctx.reserved };
        let dir = work.join(format!("remake-{i}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let made = song(&step, model, &dir).await?.into_iter().next().ok_or("The music model didn't make the part again.")?;
        // A step between: out of the gallery.
        if current.id != original.id {
            current.hidden = true;
            store::update(&ctx.state.db.lock().unwrap(), &c, &current)?;
        }
        current = made;
    }
    (ctx.progress)("Listening to the new version", spans.len() as f64 / (spans.len() as f64 + 1.0));
    let after = hear(ctx.state, &current, ctx.cancel).await?;
    current.parent = Some(original.id.clone());
    current.prompt = original.prompt.clone();
    if after.score <= review.score {
        current.note = Some("This version wasn't clearer than the one it was made from, so both are kept.".into());
    }
    current.review = Some(after);
    store::update(&ctx.state.db.lock().unwrap(), &c, &current)?;
    crate::applog::line("media", &format!("Made {} unclear part(s) of a song again: score {} -> {}", spans.len(), review.score, current.review.as_ref().map_or(0, |r| r.score)));
    Ok(vec![current])
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

async fn narrate(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let text = crate::tts::speakable(req.prompt.trim());
    if text.trim().is_empty() {
        return Err("Type the text to read aloud.".into());
    }
    let (voice, language) = (req.voice.clone(), req.language.clone().filter(|l| l != "auto"));
    let (floats, rate) = if let Some(own) = recorded_voice(req) {
        own_voice::speak(ctx.state, own, &text, language.as_deref(), ctx.cancel, ctx.progress).await?
    } else {
        (ctx.progress)("Reading", 0.3);
        let speech = tokio::task::spawn_blocking(move || crate::tts::synthesize(&text, voice.as_deref(), language.as_deref(), 1.0))
            .await
            .map_err(|e| e.to_string())??;
        (speech.samples.iter().map(|s| *s as f32 / 32768.0).collect(), speech.rate)
    };
    let wav = crate::audio::wav_encode(&floats, rate);
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "audio".into(),
        op: "narrate".into(),
        prompt: req.prompt.trim().to_string(),
        mime: "audio/wav".into(),
        seconds: floats.len() as f64 / rate.max(1) as f64,
        chat_id: req.chat_id.clone(),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &wav, None)?])
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct MediaCard {
    #[serde(flatten)]
    spec: MediaModel,
    fit: MediaFit,
    installed: bool,
}

#[derive(Serialize)]
pub struct MediaView {
    models: Vec<MediaCard>,
    /// Cloud picture, video and voice models usable now (Cloud level).
    cloud: Vec<crate::cloud::MediaChoice>,
    /// Models that can't run on this PC, left out of the list.
    hidden: usize,
    /// The same, by kind and why, so each kind's list can say so.
    hidden_by_kind: Vec<Hidden>,
    /// Why video isn't available here, when no video model fits.
    video_note: Option<String>,
    installing: Vec<String>,
    jobs: Vec<JobInfo>,
    gallery: usize,
    /// The default model per kind.
    pub defaults: MediaDefaults,
}

/// Models of a kind left out of the list, and why: "memory" (more than this
/// PC has), "card" (a bigger graphics card) or "disk" (more free space).
#[derive(Debug, Serialize, PartialEq)]
pub struct Hidden {
    kind: Kind,
    why: &'static str,
    count: usize,
}

/// Why a model that doesn't fit is left out, in one word.
fn hidden_why(m: &MediaModel, f: &MediaFit) -> &'static str {
    if f.runnable {
        "disk"
    } else if matches!(m.kind, Kind::Music | Kind::Background) || f.reason.as_deref().is_some_and(|r| r.starts_with("needs about")) {
        "memory"
    } else {
        "card"
    }
}

fn count_hidden(list: &mut Vec<Hidden>, kind: Kind, why: &'static str) {
    match list.iter_mut().find(|h| h.kind == kind && h.why == why) {
        Some(h) => h.count += 1,
        None => list.push(Hidden { kind, why, count: 1 }),
    }
}

/// The Studio's models: the ones this PC runs, or with `all`, every one
/// (those it can't run say why and can't be installed).
#[tauri::command(async)]
pub fn media_view(state: AppStateRef, all: Option<bool>) -> MediaView {
    view(&state, all.unwrap_or(false))
}

fn view(state: &AppState, all: bool) -> MediaView {
    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let list = installed(&state.db.lock().unwrap());
    let mut hidden = 0;
    let mut hidden_by_kind = Vec::new();
    let mut video_note = None;
    let mut models = Vec::new();
    let installing: Vec<String> = state.installs.lock().unwrap().keys().cloned().collect();
    for m in &self::models(state) {
        let inst = list.iter().find(|i| i.model_id == m.id);
        // From Hugging Face: shown once installed (or while it installs).
        if m.from_hub.is_some() && inst.is_none() && !installing.contains(&m.id) {
            continue;
        }
        let f = fit(m, &hw, &b, missing_bytes(&state.paths, m), inst.and_then(|i| i.secs));
        if inst.is_none() && !(f.runnable && f.disk_ok) {
            hidden += 1;
            count_hidden(&mut hidden_by_kind, m.kind, hidden_why(m, &f));
            if m.kind == Kind::Video && video_note.is_none() {
                video_note = f.reason.clone().map(|r| format!("Video isn't available on this PC: the smallest video model {r}."));
            }
            if !all {
                continue;
            }
        }
        models.push(MediaCard { spec: m.clone(), fit: f, installed: inst.is_some() });
    }
    if models.iter().any(|c| c.spec.kind == Kind::Video) {
        video_note = None;
    }
    // One lock for both reads, released before the view is built: a lock
    // taken inside the struct literal lives until the literal ends, so a
    // second one there would wait on the first forever.
    let unlocked = state.cipher().is_ok();
    let (cloud, gallery) = {
        let conn = state.db.lock().unwrap();
        let gallery = if unlocked { store::count(&conn) } else { 0 };
        (crate::cloud::media_choices(&conn), gallery)
    };
    let defaults = defaults(&state.db.lock().unwrap());
    MediaView { models, cloud, hidden, hidden_by_kind, video_note, installing, jobs: list_jobs(), gallery, defaults }
}

#[tauri::command(async)]
pub fn install_media_model(app: AppHandle, state: AppStateRef, model_id: String, anyway: Option<bool>) -> Result<(), String> {
    let spec = models(&state).into_iter().find(|m| m.id == model_id).ok_or("Unknown model")?;
    start_install_as(app, state.inner().clone(), spec, None, anyway.unwrap_or(false))
}

/// The Studio's Hugging Face search: picture, video or music models built
/// on a family SulcusAI runs.
#[tauri::command]
pub async fn media_hub_search(state: AppStateRef<'_>, kind: String, query: String, sort: String, all: Option<bool>) -> Result<hub::HubMediaResults, String> {
    hub::search(state.inner(), &kind, &query, &sort, all.unwrap_or(false)).await
}

/// Installs one file of a Hugging Face repository like a catalog model.
#[tauri::command]
pub async fn media_hub_install(app: AppHandle, state: AppStateRef<'_>, repo: String, file: String, anyway: Option<bool>) -> Result<String, String> {
    let spec = hub::prepare(state.inner(), &repo, &file).await?;
    let id = spec.id.clone();
    if let Err(e) = start_install_as(app, state.inner().clone(), spec, None, anyway.unwrap_or(false)) {
        hub::forget(&state.db.lock().unwrap(), &id).ok();
        return Err(e);
    }
    Ok(id)
}

/// Downloads a model in the background; `then_feature` is turned on after.
pub(crate) fn start_install(app: AppHandle, state: Arc<AppState>, spec: MediaModel, then_feature: Option<Feature>) -> Result<(), String> {
    start_install_as(app, state, spec, then_feature, false)
}

/// `start_install`; with `anyway`, also a model this PC probably can't run
/// (the person was told and chose to): it's tried when used, with the
/// memory check and the card's limits as always.
pub(crate) fn start_install_as(app: AppHandle, state: Arc<AppState>, spec: MediaModel, then_feature: Option<Feature>, anyway: bool) -> Result<(), String> {
    let hw = state.hardware.read().unwrap().clone();
    let f = fit(&spec, &hw, &Budget::from_hardware(&hw), missing_bytes(&state.paths, &spec), None);
    if !f.runnable && !anyway {
        return Err(format!("This PC can't run {}: it {}.", spec.name, f.reason.unwrap_or_else(|| "needs more".into())));
    }
    if !f.runnable {
        crate::applog::line("media", &format!("Installing {} though it probably won't run here ({}), as asked", spec.name, f.reason.clone().unwrap_or_default()));
    }
    if !f.disk_ok {
        return Err("There isn't enough free disk space for this model.".into());
    }
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = crate::claim(&state.installs, &spec.id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
            app.emit("install:progress", json!({ "model_id": spec.id, "phase": phase, "received": received, "total": total })).ok();
        };
        state.log("model", &format!("Started installing {}", spec.name));
        // A cancel deletes only what this install downloaded.
        let had: Vec<PathBuf> = spec.files.iter().map(|f| file_path(&state.paths, &spec, f)).filter(|p| p.exists()).collect();
        let result = install(&state, &spec, &cancel, &emit).await;
        let payload = match &result {
            Ok(()) => {
                if let Some(feat) = then_feature {
                    let _ = crate::features::set(&state.db.lock().unwrap(), feat, true);
                }
                state.log("model", &format!("Installed {}", spec.name));
                json!({ "model_id": spec.id, "name": spec.name, "ok": true })
            }
            Err(e) if e == download::CANCELLED => {
                discard_download(&state, &spec, &had);
                state.log("model", &format!("Cancelled installing {}", spec.name));
                json!({ "model_id": spec.id, "name": spec.name, "ok": false, "cancelled": true })
            }
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", spec.name));
                json!({ "model_id": spec.id, "name": spec.name, "ok": false, "error": e })
            }
        };
        app.emit("install:finished", payload).ok();
        app.emit("features:changed", json!({})).ok();
    });
    Ok(())
}

#[tauri::command]
pub async fn remove_media_model(state: AppStateRef<'_>, model_id: String) -> Result<u64, String> {
    state.cipher()?;
    remove_now(&state, &model_id)
}

/// Deletes a model's files and half-downloaded parts, except files a model
/// in `installed` still uses and those in `also_keep`. Returns the bytes freed.
fn delete_files(paths: &Paths, all: &[MediaModel], spec: &MediaModel, installed: &[InstalledMedia], also_keep: &[PathBuf]) -> u64 {
    let mut keep: Vec<PathBuf> = all
        .iter()
        .filter(|m| m.id != spec.id && installed.iter().any(|i| i.model_id == m.id))
        .flat_map(|m| m.files.iter().map(|f| file_path(paths, m, f)).collect::<Vec<_>>())
        .collect();
    keep.extend_from_slice(also_keep);
    let mut freed = 0;
    for f in &spec.files {
        let p = file_path(paths, spec, f);
        if !keep.contains(&p) {
            freed += p.metadata().map_or(0, |m| m.len());
            std::fs::remove_file(&p).ok();
        }
        let part = download::part_path(&p);
        freed += part.metadata().map_or(0, |m| m.len());
        std::fs::remove_file(&part).ok();
    }
    freed
}

/// After a cancelled install: deletes what it downloaded (files that were
/// here before it started, `had`, and files another installed model shares
/// stay), and forgets a Hugging Face model that was never installed. A model
/// that was already installed keeps everything.
fn discard_download(state: &AppState, spec: &MediaModel, had: &[PathBuf]) {
    let all = models(state);
    let conn = state.db.lock().unwrap();
    let list = installed(&conn);
    if list.iter().any(|i| i.model_id == spec.id) {
        return;
    }
    let freed = delete_files(&state.paths, &all, spec, &list, had);
    if spec.from_hub.is_some() {
        hub::forget(&conn, &spec.id).ok();
    }
    crate::applog::line("download", &format!("{}: cancelled, deleted {} it had downloaded", spec.name, crate::size_label(freed)));
}

/// Removes a picture, video or music model from this PC: its files (and any
/// half-downloaded ones), unless another installed model shares them, and
/// what SulcusAI kept about it. Returns the bytes freed.
pub(crate) fn remove_now(state: &AppState, model_id: &str) -> Result<u64, String> {
    let all = models(state);
    let spec = all.iter().find(|m| m.id == model_id).ok_or("Unknown model")?.clone();
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    list.retain(|i| i.model_id != model_id);
    let freed = delete_files(&state.paths, &all, &spec, &list, &[]);
    if spec.kind == Kind::Background {
        cutout::unload();
    }
    save_installed(&conn, &list)?;
    if spec.from_hub.is_some() {
        hub::forget(&conn, &spec.id)?;
    }
    // Not the default any more.
    let mut d = defaults(&conn);
    if [&d.image, &d.video, &d.music].iter().any(|x| x.as_deref() == Some(model_id)) {
        for slot in [&mut d.image, &mut d.video, &mut d.music] {
            if slot.as_deref() == Some(model_id) {
                *slot = None;
            }
        }
        db::set(&conn, DEFAULTS_KEY, &d)?;
    }
    db::log_action(&conn, "model", &format!("Removed {} and deleted its files ({} freed)", spec.name, crate::size_label(freed)));
    Ok(freed)
}

/// Tells the window a job finished (its card closes; what it made shows).
pub(crate) fn job_done(state: &AppState, job_id: &str, result: &Result<Vec<MediaItem>, String>) {
    let payload = match result {
        Ok(items) => json!({ "job_id": job_id, "ok": true, "items": items }),
        Err(e) if e == download::CANCELLED => json!({ "job_id": job_id, "ok": false, "cancelled": true }),
        Err(e) => json!({ "job_id": job_id, "ok": false, "error": e }),
    };
    emit(state, "media:done", payload);
}

#[tauri::command(async)]
pub fn media_start(state: AppStateRef, request: Request) -> Result<String, String> {
    state.cipher()?;
    // A job for the compute PC is checked there.
    if !crate::sync::compute::media_here(&state, &request) {
        check(&state, &request)?;
    }
    let id = store::new_id();
    let (state, job_id) = (state.inner().clone(), id.clone());
    tauri::async_runtime::spawn(async move {
        let result = run(&state, job_id.clone(), request).await;
        job_done(&state, &job_id, &result);
    });
    Ok(id)
}

/// What the prompt writer knows about a picture model: its family, whether
/// it was tuned on this PC (and how well that went), and which models write
/// and check its pictures.
#[tauri::command]
pub async fn media_prompt_status(state: AppStateRef<'_>, model_id: String) -> Result<serde_json::Value, String> {
    state.cipher()?;
    let m = models(&state).into_iter().find(|m| m.id == model_id).ok_or("That model isn't in the list.")?;
    let fam = prompts::family(&m);
    let tuned = prompts::program(&state.paths, fam).tuned;
    let own = installed_file(&state, &m, "llm").is_some();
    let own_eyes = own && installed_file(&state, &m, "llm_vision").is_some();
    let reader = if own_eyes { Some(m.name.clone()) } else { crate::vision::readers(&state).into_iter().next().map(|r| r.name) };
    let learned = learn::summary(&learn::load(&state, &m));
    Ok(json!({ "family": fam, "tuned": tuned, "own_writer": own, "reader": reader, "coach": coach::trained(&state, &m), "learned": learned, "learning": learn::on(&state) }))
}

/// Goes back to the prompt writer's bundled instructions for a model's family.
#[tauri::command(async)]
pub fn media_forget_tuning(state: AppStateRef, model_id: String) -> Result<(), String> {
    state.cipher()?;
    let m = models(&state).into_iter().find(|m| m.id == model_id).ok_or("That model isn't in the list.")?;
    prompts::forget_tuning(&state.paths, prompts::family(&m));
    coach::forget(&state, &m);
    learn::forget(&state, &m);
    db::log_action(&state.db.lock().unwrap(), "model", &format!("Forgot the prompt writer's tuning for {}", m.name));
    Ok(())
}

#[tauri::command(async)]
pub fn media_cancel(job_id: String) {
    cancel(&job_id);
}

#[tauri::command(async)]
pub fn media_jobs() -> Vec<JobInfo> {
    list_jobs()
}

#[tauri::command(async)]
pub fn media_list(state: AppStateRef, kind: Option<String>, before: Option<i64>, limit: Option<usize>) -> Result<Vec<MediaItem>, String> {
    let c = state.cipher()?;
    Ok(store::list(&state.db.lock().unwrap(), &c, kind.as_deref(), limit.unwrap_or(60).min(500), before))
}

/// The Studio's "Improve": the chat model (on this PC unless the default is
/// a cloud one) turns a short idea into a fuller description.
#[tauri::command]
pub async fn improve_media_prompt(state: AppStateRef<'_>, prompt: String, kind: String) -> Result<String, String> {
    state.cipher()?;
    let idea = prompt.trim();
    if idea.is_empty() {
        return Err("Write an idea first.".into());
    }
    let guide = match kind.as_str() {
        "video" => "a short video clip: the subject, what happens, how the camera moves, the light and mood",
        "music" => "a piece of music: the genre, mood, instruments, tempo and voice (if any)",
        _ => "a picture: the subject, setting, composition, light and mood",
    };
    let (ep, _, _) = crate::llm_endpoint(state.inner(), None, false, |_| {}).await?;
    let messages = vec![
        json!({ "role": "system", "content": format!(
            "You write descriptions for a model that makes media. Rewrite the person's idea as one vivid, specific description of {guide}. \
             Keep everything they asked for, exactly: every count, color, position and relation, and any words to show in quotes, spelled as given. \
             Add nothing that changes it. If they named a style, keep it. Write it in English, in plain sentences with no lists or explanations: \
             about 40 words for a simple idea, up to 120 for a scene with many parts."
        ) }),
        json!({ "role": "user", "content": idea }),
    ];
    let better = crate::chat::complete(&ep, messages, json!({}), 400).await?;
    let better = better.trim().trim_matches('"').trim();
    Ok(if better.is_empty() { idea.to_string() } else { better.to_string() })
}

/// A chat's pictures, video and music: made there or attached.
#[tauri::command(async)]
pub fn media_in_chat(state: AppStateRef, chat_id: String) -> Result<Vec<MediaItem>, String> {
    let c = state.cipher()?;
    Ok(store::in_chat(&state.db.lock().unwrap(), &c, &chat_id))
}

#[tauri::command(async)]
pub fn media_get(state: AppStateRef, id: String) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    store::get(&state.db.lock().unwrap(), &c, &id).ok_or_else(|| "That item is no longer in the gallery.".into())
}

#[tauri::command(async)]
pub fn media_file(state: AppStateRef, id: String) -> Result<tauri::ipc::Response, String> {
    let c = state.cipher()?;
    Ok(tauri::ipc::Response::new(store::read(&state.paths, &c, &id)?))
}

#[tauri::command(async)]
pub fn media_thumb(state: AppStateRef, id: String) -> Result<tauri::ipc::Response, String> {
    let c = state.cipher()?;
    Ok(tauri::ipc::Response::new(store::read_thumb(&state.paths, &c, &id)?))
}

/// A picture kept as a song's cover art (its thumbnail, larger than a
/// tile's so the song's page can show it big).
fn set_cover(state: &AppState, c: &crate::crypto::Cipher, song_id: &str, img: &image::DynamicImage) -> Result<MediaItem, String> {
    let conn = state.db.lock().unwrap();
    let mut song = store::get(&conn, c, song_id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Cover art is for songs.".into());
    }
    let side = img.width().min(img.height());
    let square = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side).thumbnail(768, 768).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 86).encode_image(&square).map_err(|e| e.to_string())?;
    store::write_thumb(&state.paths, c, song_id, &jpeg)?;
    song.poster = true;
    store::update(&conn, c, &song)?;
    Ok(song)
}

/// Queues cover art for a song just made, when a picture model is here.
fn queue_cover_art(state: &Arc<AppState>, song: &MediaItem) {
    if choose(state, "generate", None, false).is_err() {
        crate::applog::line("media", "No cover art: no picture model is installed");
        return;
    }
    let req = Request { op: "cover_art".into(), source: Some(song.id.clone()), chat_id: song.chat_id.clone(), ..Default::default() };
    let (state, id) = (state.clone(), store::new_id());
    tauri::async_runtime::spawn(async move {
        let result = run(&state, id.clone(), req).await;
        job_done(&state, &id, &result);
    });
}

/// What a song's cover art is asked to show: the person's words for it,
/// else the song's own description. Never "album cover": with those words
/// Z-Image Turbo lettered a title across the top.
fn art_prompt(song: &MediaItem, asked: &str) -> String {
    let what = if asked.trim().is_empty() { song.prompt.trim().trim_end_matches('.').to_string() } else { asked.trim().trim_end_matches('.').to_string() };
    format!("A striking square artwork that captures the feeling of this: {what}. Cinematic light, bold composition, rich colour, one clear subject. The image has no text anywhere.")
}

/// What cover art leaves out, for picture models that take a negative prompt.
const ART_NEGATIVE: &str = "text, letters, words, title, typography, logo, watermark, signature";

/// Cover art for a song (`req.source`): a square picture made by the
/// picture model and kept as the song's picture; the picture itself isn't
/// kept in the gallery.
async fn cover_art(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let id = ctx.req.source.as_deref().ok_or("Pick a song first.")?;
    let c = ctx.state.work_cipher()?;
    let song = store::get(&ctx.state.db.lock().unwrap(), &c, id).ok_or("That song is no longer in the gallery.")?;
    if song.kind != "audio" {
        return Err("Cover art is for songs.".into());
    }
    let req = Request {
        op: "generate".into(),
        model_id: Some(m.id.clone()),
        prompt: art_prompt(&song, &ctx.req.prompt),
        negative: Some(ART_NEGATIVE.into()),
        shape: Some("square".into()),
        count: Some(1),
        seed: ctx.req.seed,
        ..Default::default()
    };
    let step = RunCtx { state: ctx.state, req: &req, cancel: ctx.cancel, progress: ctx.progress, reserved: ctx.reserved };
    let pic = picture(&step, m, work).await?.into_iter().next().ok_or("The picture model didn't make the cover.")?;
    let bytes = store::read(&ctx.state.paths, &c, &pic.id)?;
    let _ = store::delete(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &pic.id);
    let img = imaging::decode(&bytes)?;
    Ok(vec![set_cover(ctx.state, &c, &song.id, &img)?])
}

/// A captioned video's captions saved as a subtitle file where the person
/// chose (players and editors read .srt).
#[tauri::command(async)]
pub fn media_save_captions(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    let c = state.work_cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That video is no longer in the gallery.")?;
    let text = item.lyrics.filter(|t| t.contains("-->")).ok_or("That video has no captions.")?;
    let path = std::path::PathBuf::from(path);
    let path = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt")) { path } else { path.with_extension("srt") };
    std::fs::write(&path, text).map_err(|e| format!("Couldn't save the captions: {e}"))
}

/// A picture of the person's own, chosen in the window, as a song's cover.
#[tauri::command(async)]
pub fn media_set_cover(state: AppStateRef, id: String, path: String) -> Result<MediaItem, String> {
    let c = state.work_cipher()?;
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read that picture: {e}"))?;
    let img = imaging::decode(&bytes).map_err(|_| "That file isn't a picture SulcusAI can read (PNG, JPEG, WebP, GIF or BMP).".to_string())?;
    set_cover(&state, &c, &id, &img)
}

/// Saves a still frame the window took from a video as its thumbnail
/// (the window can decode WebM; the core can't).
#[tauri::command(async)]
pub fn media_set_poster(state: AppStateRef, id: String, data: String) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| "That still isn't readable.".to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|_| "That still isn't a picture.".to_string())?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is gone.")?;
    if item.kind != "video" {
        return Err("Only videos take a still.".into());
    }
    store::write_thumb(&state.paths, &c, &id, &imaging::thumbnail(&img)?)?;
    item.poster = true;
    store::update(&conn, &c, &item)?;
    Ok(item)
}

#[tauri::command(async)]
pub fn media_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    // A song sung in a recorded voice takes the version it was made from,
    // kept out of the gallery for it, along.
    if let Some(parent) = store::get(&conn, &c, &id).and_then(|i| i.parent.zip(Some(i.op))).and_then(|(p, op)| store::get(&conn, &c, &p).filter(|p| p.hidden && p.op == op && p.kind == "audio")) {
        store::delete(&conn, &state.paths, &parent.id)?;
    }
    library::forget_song(&conn, &c, &id);
    store::delete(&conn, &state.paths, &id)?;
    db::log_action(&conn, "media", "Deleted an item from the gallery");
    Ok(())
}

#[tauri::command(async)]
pub fn media_favorite(state: AppStateRef, id: String, on: bool) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is no longer in the gallery.")?;
    item.favorite = on;
    item.disliked &= !on;
    store::update(&conn, &c, &item)?;
    Ok(item)
}

/// A thumbs down (or taking it back): a liked song stops being a favorite.
#[tauri::command(async)]
pub fn media_dislike(state: AppStateRef, id: String, on: bool) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is no longer in the gallery.")?;
    item.disliked = on;
    item.favorite &= !on;
    store::update(&conn, &c, &item)?;
    Ok(item)
}

#[tauri::command(async)]
pub fn music_playlists(state: AppStateRef) -> Result<Vec<library::Playlist>, String> {
    let c = state.cipher()?;
    Ok(library::list(&state.db.lock().unwrap(), &c))
}

/// Saves a playlist (new without an id): its name and songs in order.
#[tauri::command(async)]
pub fn music_playlist_save(state: AppStateRef, playlist: library::Playlist) -> Result<library::Playlist, String> {
    let c = state.cipher()?;
    library::save(&state.db.lock().unwrap(), &c, playlist)
}

/// What the person likes in songs, from their liked, made and "not for me"
/// songs; or what some songs share (a playlist's, each counted as liked).
#[tauri::command(async)]
pub fn music_taste(state: AppStateRef, ids: Option<Vec<String>>) -> Result<library::Taste, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let items = match ids {
        Some(ids) => ids.iter().filter_map(|id| store::get(&conn, &c, id)).map(|i| MediaItem { favorite: !i.disliked, ..i }).collect(),
        None => store::list(&conn, &c, Some("audio"), 5000, None),
    };
    Ok(library::taste(&items))
}

#[tauri::command(async)]
pub fn music_playlist_delete(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    library::remove(&state.db.lock().unwrap(), &c, &id)
}

/// Names a song (or anything in the gallery); empty goes back to none.
#[tauri::command(async)]
pub fn media_set_title(state: AppStateRef, id: String, title: String) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is no longer in the gallery.")?;
    item.title = Some(title.trim().chars().take(120).collect::<String>()).filter(|t| !t.is_empty());
    store::update(&conn, &c, &item)?;
    Ok(item)
}

/// Saves a copy, unencrypted, where the user picked (Save as…).
#[tauri::command(async)]
pub fn media_export(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    let c = state.cipher()?;
    let bytes = store::read(&state.paths, &c, &id)?;
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("media", "Saved a copy of a gallery item outside the app");
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Screen {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Top-left corner on the desktop (physical pixels).
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

/// The screens connected now, at their real resolutions.
#[tauri::command]
pub fn wallpaper_screens(app: AppHandle) -> Vec<Screen> {
    let primary = app.primary_monitor().ok().flatten().map(|m| *m.position());
    app.available_monitors()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, m)| Screen {
            name: m.name().cloned().filter(|n| !n.trim().is_empty() && !n.starts_with(r"\\.\")).unwrap_or_else(|| format!("Screen {}", i + 1)),
            width: m.size().width,
            height: m.size().height,
            x: m.position().x,
            y: m.position().y,
            primary: primary == Some(*m.position()),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct WallpaperSet {
    /// The copy Windows shows (Pictures\SulcusAI Wallpapers).
    pub path: String,
    /// "windows" (set), "lively" (handed to Lively Wallpaper) or "saved"
    /// (a moving one, and Lively isn't installed).
    pub how: String,
}

/// Sets a gallery picture (or looping clip) as the desktop wallpaper, on the
/// screen at `x`,`y` or on all of them. A copy goes to Pictures\SulcusAI
/// Wallpapers, since Windows shows a file, not the encrypted gallery.
#[tauri::command(async)]
pub fn wallpaper_set(state: AppStateRef, id: String, x: Option<i32>, y: Option<i32>) -> Result<WallpaperSet, String> {
    let c = state.cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That's no longer in the gallery.")?;
    let ext = match item.mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => return Err("Only pictures and clips can be wallpapers.".into()),
    };
    let path = wallpaper::folder()?.join(format!("wallpaper-{}.{ext}", &id[..id.len().min(12)]));
    std::fs::write(&path, store::read(&state.paths, &c, &id)?).map_err(|e| format!("Couldn't save the wallpaper: {e}"))?;
    let how = if item.kind == "image" {
        wallpaper::set_still(&path, x.zip(y))?;
        "windows"
    } else {
        match wallpaper::set_moving(&path) {
            Ok(()) => "lively",
            Err(e) if e == "NO_LIVELY" => "saved",
            Err(e) => return Err(e),
        }
    };
    db::log_action(&state.db.lock().unwrap(), "media", "Set a desktop wallpaper from the gallery");
    Ok(WallpaperSet { path: path.to_string_lossy().into_owned(), how: how.into() })
}

fn mime_for(name: &str) -> Option<(&'static str, &'static str)> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ("image", "image/png"),
        "jpg" | "jpeg" => ("image", "image/jpeg"),
        "webp" => ("image", "image/webp"),
        "webm" => ("video", "video/webm"),
        "mp4" => ("video", "video/mp4"),
        "mp3" => ("audio", "audio/mpeg"),
        "wav" => ("audio", "audio/wav"),
        "ogg" => ("audio", "audio/ogg"),
        // Songs to work on: Windows reads these for the music engine.
        "m4a" => ("audio", "audio/mp4"),
        "aac" => ("audio", "audio/aac"),
        "flac" => ("audio", "audio/flac"),
        "wma" => ("audio", "audio/x-ms-wma"),
        _ => return None,
    })
}

const MAX_IMPORT: u64 = 200 * 1024 * 1024;

/// Brings a picture, clip or sound from a file into the gallery.
#[tauri::command(async)]
pub fn media_import(state: AppStateRef, path: String, chat_id: Option<String>) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let (kind, mime) = mime_for(&path).ok_or("Pick a picture (PNG, JPEG, WebP), a clip (WebM, MP4) or a sound or song (MP3, WAV, M4A, AAC, FLAC, OGG, WMA).")?;
    // Attached to a chat: a hidden attachment, deleted with the chat. Songs
    // too, for the assistant to work on.
    if chat_id.is_some() && kind == "video" {
        return Err("Only pictures and songs can be attached to a chat.".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_IMPORT {
        return Err("That file is too big (200 MB at most).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let hidden = chat_id.is_some();
    let mut item = add(&state, &c, bytes, kind, mime, if hidden { "attach" } else { "import" }, &name, None, chat_id, hidden)?;
    // A song's length, for picking a part of it to work on.
    if kind == "audio" {
        if let Some(secs) = audio_in::length(std::path::Path::new(&path)).filter(|s| *s > 0.0) {
            item.seconds = secs;
            store::update(&state.db.lock().unwrap(), &c, &item)?;
        }
    }
    Ok(item)
}

/// The artists the person saved.
#[tauri::command]
pub async fn music_personas(state: AppStateRef<'_>) -> Result<Vec<persona::Persona>, String> {
    let c = state.cipher()?;
    Ok(persona::list(&state.db.lock().unwrap(), &c))
}

/// Saves an artist (a new one without an id).
#[tauri::command]
pub async fn music_persona_save(state: AppStateRef<'_>, persona: persona::Persona) -> Result<persona::Persona, String> {
    let c = state.cipher()?;
    persona::save(&state.db.lock().unwrap(), &c, persona)
}

#[tauri::command]
pub async fn music_persona_delete(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    persona::remove(&state.db.lock().unwrap(), &c, &id)
}

/// An artist's style for the Studio's "Use their style": the genre, voice,
/// instruments and tempo to fill in, and their sound and way of writing.
#[tauri::command]
pub async fn music_artist_style(state: AppStateRef<'_>, name: String) -> Result<artist::Style, String> {
    state.cipher()?;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("Type the name of a singer or band.".into());
    }
    match artist::look_up(state.inner(), name).await? {
        Some(style) => Ok(style),
        None => Err(format!("The chat model doesn't know {name}. Describe their style in your own words instead.")),
    }
}

/// Lyrics for a song, written before it's made so the person can read and
/// change them (the Studio's "Write lyrics for me").
#[tauri::command]
pub async fn music_write_lyrics(state: AppStateRef<'_>, prompt: String, music: Option<MusicSpec>, seconds: Option<f32>, language: Option<String>) -> Result<String, String> {
    state.cipher()?;
    let req = Request { op: "music".into(), prompt, music, ..Default::default() };
    // "Sing as …" in the description, or "Sound like" not yet looked up.
    let styled = artist::resolve(state.inner(), &req, &|_, _| {}).await;
    let req = styled.unwrap_or(req);
    let words = song_caption(&req);
    if words.trim().is_empty() {
        return Err("Describe the song first (or pick its style), so the lyrics fit it.".into());
    }
    let spec = req.music.clone().unwrap_or_default();
    // No length (Auto): one named in the description, else the sections', else the writer's choice.
    let seconds = seconds.map(|s| s.clamp(10.0, 600.0)).or_else(|| lyrics::named_length(&req.prompt));
    lyrics::write(state.inner(), &words, seconds, language.as_deref().filter(|l| *l != "auto"), &spec.sections(), &spec.lyrics_guide(), spec.flow(&req.prompt))
        .await
        .map_err(|why| match why {
            lyrics::NotWritten::NoModel(_) => "There's no chat model on this PC to write lyrics. Install one from Models, or type your own.".to_string(),
            lyrics::NotWritten::Unusable(e) => format!("The lyrics didn't come out right ({e}). Try again, or type your own."),
        })
}

/// Some lines of the lyrics (`part`, from `whole`) written again by the chat
/// model, changed as `how` says (the Studio's "Write these lines again").
#[tauri::command]
pub async fn music_rewrite_lines(state: AppStateRef<'_>, whole: String, part: String, how: String, language: Option<String>) -> Result<String, String> {
    state.cipher()?;
    lyrics::rewrite(state.inner(), &whole, &part, &how, language.as_deref()).await
}

pub(crate) fn add(state: &AppState, c: &crate::crypto::Cipher, bytes: Vec<u8>, kind: &str, mime: &str, op: &str, prompt: &str, parent: Option<String>, chat_id: Option<String>, hidden: bool) -> Result<MediaItem, String> {
    let mut item = MediaItem { kind: kind.into(), op: op.into(), prompt: prompt.into(), mime: mime.into(), parent, chat_id, hidden, ..Default::default() };
    let mut thumb = None;
    let mut bytes = bytes;
    if kind == "image" {
        let img = imaging::decode(&bytes)?;
        (item.width, item.height) = img.dimensions();
        thumb = Some(imaging::thumbnail(&img)?);
        // WebP and JPEG are kept as they are; anything else becomes PNG.
        if !matches!(mime, "image/png" | "image/jpeg" | "image/webp") {
            bytes = imaging::png(&img)?;
            item.mime = "image/png".into();
        }
    }
    store::save(&state.db.lock().unwrap(), &state.paths, c, item, &bytes, thumb.as_deref())
}

/// Adds bytes made in the window (a trimmed sound, a pasted or attached
/// picture). `hidden` keeps chat attachments out of the studio.
#[tauri::command(async)]
pub fn media_add(
    state: AppStateRef,
    data: String,
    mime: String,
    op: String,
    prompt: Option<String>,
    parent: Option<String>,
    chat_id: Option<String>,
    hidden: Option<bool>,
    seconds: Option<f64>,
    lyrics: Option<String>,
    note: Option<String>,
) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| "That file couldn't be read.".to_string())?;
    if bytes.len() as u64 > MAX_IMPORT {
        return Err("That file is too big (200 MB at most).".into());
    }
    let kind = match mime.split('/').next() {
        Some("image") => "image",
        Some("video") => "video",
        Some("audio") => "audio",
        _ => return Err("Only pictures, clips and sounds can be added.".into()),
    };
    let op = if matches!(op.as_str(), "trim" | "paste" | "attach" | "screenshot" | "import" | "studio" | "recording") { op } else { "import".into() };
    let mut item = add(&state, &c, bytes, kind, &mime, &op, prompt.as_deref().unwrap_or(""), parent, chat_id, hidden.unwrap_or(false))?;
    // What was done to it in the window ("Faded out over 8 s").
    item.note = note.map(|n| n.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty());
    if seconds.is_some() || lyrics.is_some() || item.note.is_some() {
        if let Some(s) = seconds {
            item.seconds = s;
        }
        // A song with a section cut out or added keeps its words.
        if let Some(l) = lyrics.filter(|l| kind == "audio" && lyrics::has_singing(l)) {
            item.lyrics = Some(l);
        }
        store::update(&state.db.lock().unwrap(), &c, &item)?;
    }
    Ok(item)
}

/// A song's map (`songmap`) saved as a MIDI file, a track a part: the one
/// given (notes edited in the song view), else the song's own.
#[tauri::command]
pub fn media_song_midi(state: AppStateRef, id: String, path: String, map: Option<songmap::SongMap>) -> Result<(), String> {
    let c = state.cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That song is no longer in the gallery.")?;
    let map = map.or(item.map).ok_or("See how the song is built first.")?;
    std::fs::write(&path, songmap::midi(&map)).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("media", "Saved a song's notes as a MIDI file");
    Ok(())
}

/// Keeps the notes as edited in the song view on the song (its sound
/// doesn't change: the notes are read from it, not played into it).
#[tauri::command]
pub fn media_song_map_save(state: AppStateRef, id: String, map: songmap::SongMap) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That song is no longer in the gallery.")?;
    if item.kind != "audio" || map.parts.iter().map(|p| p.notes.len()).sum::<usize>() > 200_000 {
        return Err("Those notes can't be kept with this song.".into());
    }
    item.map = Some(map);
    store::update(&conn, &c, &item)?;
    Ok(item)
}

// ---------- compose ----------

/// The sampled instruments Compose plays its notes on, downloaded the first
/// time (`compose:getting` says how far), as bytes for the window's player.
#[tauri::command]
pub async fn compose_sounds(state: AppStateRef<'_>) -> Result<tauri::ipc::Response, String> {
    crate::features::require(&state, Feature::Music)?;
    let h = state.catalog.media.instruments.clone().ok_or("The instruments aren't available.")?;
    let st = state.inner().clone();
    let cancel = AtomicBool::new(false);
    // A message a percent: one a download chunk (thousands) left the window minutes behind.
    let shown = std::sync::atomic::AtomicU32::new(u32::MAX);
    let path = fetch_helper(&st, &h, "Getting the instruments (once)", &cancel, &|_, f| {
        let pct = (f * 100.0) as u32;
        if shown.swap(pct, Ordering::Relaxed) != pct {
            emit(&st, "compose:getting", json!({ "fraction": f }));
        }
    })
    .await?;
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read the instruments: {e}"))?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// A song written as notes: the chat model plans it, the arranger writes
/// the notes (`compose`).
#[tauri::command]
pub async fn compose_write(state: AppStateRef<'_>, mut ask: compose::Ask) -> Result<compose::Composition, String> {
    crate::features::require(&state, Feature::Music)?;
    if ask.description.trim().is_empty() {
        return Err("Describe the song first.".into());
    }
    if ask.beats == 0 || ask.unit == 0 {
        (ask.beats, ask.unit) = (4, 4);
    }
    if ask.sections == 0 {
        ask.sections = 6;
    }
    if ask.seed == 0 {
        ask.seed = random_seed() as u64;
    }
    let plan = compose::write_plan(state.inner(), &ask).await?;
    state.log("media", &format!("Wrote a song as notes: {} sections in {}", plan.sections.len(), plan.time()));
    Ok(compose::Composition { parts: compose::arrange(&plan), plan })
}

/// Every note written again from the plan (after a new time signature).
#[tauri::command(async)]
pub fn compose_arrange(plan: compose::Plan) -> compose::Composition {
    let plan = compose::tidy(plan);
    compose::Composition { parts: compose::arrange(&plan), plan }
}

/// A section changed (`change`: in the person's words, by the chat model),
/// written again, added, copied or taken out; the other notes stay.
#[tauri::command]
pub async fn compose_section(state: AppStateRef<'_>, composition: compose::Composition, index: usize, op: compose::SectionOp, change: Option<String>) -> Result<compose::Composition, String> {
    let op = match change.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(words) => compose::SectionOp::Set { section: compose::edit_section(state.inner(), &composition.plan, index, words).await? },
        None => op,
    };
    compose::apply(&composition, index, op)
}

/// Keeps a composition in the gallery with its demo (WAV, base64, played in
/// the window): a new item, or the one it was opened from.
#[tauri::command(async)]
pub fn compose_save(state: AppStateRef, id: Option<String>, composition: compose::Composition, audio: String, seconds: f64) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(audio.trim()).map_err(|_| "The demo couldn't be read.".to_string())?;
    if bytes.len() as u64 > MAX_IMPORT || composition.parts.iter().map(|p| p.notes.len()).sum::<usize>() > 200_000 {
        return Err("That song is too big to keep.".into());
    }
    let words = composition.plan.lyrics();
    let lyrics = Some(words).filter(|l| lyrics::has_singing(l));
    let prompt = if composition.plan.description.is_empty() { composition.plan.title.clone() } else { composition.plan.description.clone() };
    let existing = id.and_then(|id| store::get(&state.db.lock().unwrap(), &c, &id)).filter(|i| i.op == "compose");
    let mut item = match existing {
        Some(mut item) => {
            store::write_file(&state.paths, &c, &item.id, &bytes)?;
            item.size = bytes.len() as u64;
            item
        }
        None => add(&state, &c, bytes, "audio", "audio/wav", "compose", &prompt, None, None, false)?,
    };
    item.seconds = if seconds > 0.0 { seconds } else { composition.plan.seconds() };
    item.prompt = prompt;
    item.lyrics = lyrics;
    item.composition = Some(composition);
    store::update(&state.db.lock().unwrap(), &c, &item)?;
    Ok(item)
}

/// A composition as a MIDI file: saved at `path`, else its bytes (what the
/// window's player plays).
#[tauri::command(async)]
pub fn compose_midi(state: AppStateRef, composition: compose::Composition, path: Option<String>) -> Result<tauri::ipc::Response, String> {
    let bytes = compose::midi(&composition);
    let Some(path) = path else {
        return Ok(tauri::ipc::Response::new(bytes));
    };
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("media", "Saved a composition as a MIDI file");
    Ok(tauri::ipc::Response::new(Vec::new()))
}

/// "Make it real": the music model records a song that follows the
/// composition's demo (a cover of it: its tempo, chords and tune) and sings
/// its words. A new song in the gallery; the composition stays as it is.
/// `strength`: how closely it follows the demo (0 to 1).
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn compose_make_real(
    state: AppStateRef,
    id: String,
    strength: Option<f32>,
    model_id: Option<String>,
    chat_id: Option<String>,
    song: Option<MusicSpec>,
    lyrics: Option<String>,
    language: Option<String>,
) -> Result<String, String> {
    let c = state.cipher()?;
    let item = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("Keep the song first.")?;
    let plan = item.composition.as_ref().map(|x| x.plan.clone()).ok_or("That isn't a composition.")?;
    let request = real_request(&plan, &id, strength, model_id, chat_id, song, lyrics, language);
    media_start(state, request)
}

/// The music job that makes a composition real (`compose_make_real`): every
/// song option picked (in `song`, else the ones it was written with) and
/// the words given (else its own), with the composition's tempo, key and
/// time signature, as a cover of its demo.
#[allow(clippy::too_many_arguments)]
pub(crate) fn real_request(
    plan: &compose::Plan,
    id: &str,
    strength: Option<f32>,
    model_id: Option<String>,
    chat_id: Option<String>,
    song: Option<MusicSpec>,
    lyrics: Option<String>,
    language: Option<String>,
) -> Request {
    let words = lyrics.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).unwrap_or_else(|| plan.lyrics());
    let sung = lyrics::has_singing(&words);
    let ins = &plan.instruments;
    let filled = |o: Option<String>| o.filter(|s| !s.trim().is_empty());
    let mut spec = song.or_else(|| plan.song.clone()).unwrap_or_default();
    // The parts as the music model knows them, unless instruments were
    // picked; the tune is sung when there are words.
    if spec.instruments.is_empty() {
        let mut list = vec![ins.chords.clone(), ins.bass.clone(), if ins.drums.contains("kit") { "drums".into() } else { ins.drums.clone() }];
        if !sung {
            list.push(ins.melody.clone());
        }
        spec.instruments = list.into_iter().filter(|i| !i.trim().is_empty()).collect();
    }
    spec.genre = filled(spec.genre).or_else(|| filled(Some(plan.style.clone())));
    spec.era = filled(spec.era).or_else(|| filled(Some(plan.era.clone())));
    spec.voice = if sung { filled(spec.voice).or_else(|| filled(Some(plan.voice.clone()))) } else { None };
    // The composition's own tempo, key, time signature and shape.
    spec.task = Some("cover".into());
    spec.audio = Some(id.to_string());
    spec.bpm = Some(plan.bpm);
    spec.key = Some(plan.key.clone());
    spec.time_signature = Some(plan.time());
    spec.strength = Some(strength.unwrap_or(if sung { REAL_STRENGTH_SUNG } else { REAL_STRENGTH_PLAYED }).clamp(0.0, 1.0));
    spec.sections.clear();
    spec.structure = None;
    spec.auto_length = false;
    spec.harmony_layer = false;
    spec.region = None;
    spec.extend_by = None;
    spec.track = None;
    let prompt = if plan.description.trim().is_empty() { format!("{} song", plan.style) } else { plan.description.clone() };
    Request {
        op: "music".into(),
        model_id,
        prompt,
        lyrics: sung.then_some(words),
        instrumental: Some(!sung),
        language: filled(language).filter(|l| l != "auto").or_else(|| plan.language.clone()),
        chat_id,
        music: Some(spec),
        ..Default::default()
    }
}

/// How closely a composition made real follows its demo, unless asked. The
/// engine follows the demo for `(steps × strength)` of its steps, rounded
/// down (8 for the turbo models: 0.5 and 0.6 are the same song), so the
/// window offers eighths. A sung song follows less closely (the demo has no
/// real voice; with thin lyrics Whisper heard 47 of 104 words at 3 steps and
/// 1 at 4), an instrumental more (its note transitions were better at 4).
pub(crate) const REAL_STRENGTH_SUNG: f32 = 0.375;
pub(crate) const REAL_STRENGTH_PLAYED: f32 = 0.5;

/// A screenshot of the screen the mouse is on, kept as a hidden attachment.
pub fn screenshot(state: &AppState, chat_id: Option<String>) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let shot = screen::for_model(screen::capture()?);
    let img = image::DynamicImage::ImageRgba8(shot);
    let png = imaging::png(&img)?;
    let (w, h) = img.dimensions();
    let item = MediaItem { kind: "image".into(), op: "screenshot".into(), mime: "image/png".into(), width: w, height: h, chat_id, hidden: true, ..Default::default() };
    let thumb = imaging::thumbnail(&img)?;
    let item = store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &png, Some(&thumb))?;
    state.log("privacy", "Took a screenshot for a question");
    Ok(item)
}

/// A picture for a vision model: JPEG under ~1.5 megapixels, as a data URL.
pub fn image_data_url(state: &AppState, c: &crate::crypto::Cipher, id: &str) -> Result<String, String> {
    let bytes = store::read(&state.paths, c, id)?;
    let img = imaging::decode(&bytes)?;
    let (w, h) = img.dimensions();
    let (tw, th) = imaging::work_size(w, h, 1536 * 1024);
    let small = if (tw, th) == (w, h) { img } else { img.resize_exact(tw, th, image::imageops::FilterType::Triangle) };
    let mut jpg = Vec::new();
    let rgb = image::DynamicImage::ImageRgb8(small.to_rgb8());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 88).encode_image(&rgb).map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
}

#[cfg(test)]
mod tests {
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
        let state = crate::e2e::temp_state(None);
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
}

/// What FFprobe finds in a file, for tests.
#[cfg(test)]
pub(crate) async fn ffmpeg_probe_for_test(ffprobe: &std::path::Path, file: &std::path::Path) -> ffmpeg::Probe {
    ffmpeg::probe(ffprobe, file).await.unwrap()
}

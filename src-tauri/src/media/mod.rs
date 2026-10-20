// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures, video, music and narration, made on this PC.
//!
//! stable-diffusion.cpp makes and edits pictures, upscales them and makes
//! video clips; acestep.cpp makes music; BiRefNet on ONNX Runtime removes
//! backgrounds; narration uses the app's own voices. Each job runs the
//! engine program once, within the performance limits, one job at a time,
//! and what it makes goes into the gallery, encrypted.

pub mod clarify;
pub mod coach;
mod cutout;
pub(crate) mod imaging;
mod cloud;
pub mod hub;
pub mod learn;
pub(crate) mod lyrics;
mod audio_in;
pub(crate) mod music;
pub(crate) mod proc;
pub mod prompts;
pub mod screen;
mod sd;
pub mod store;
pub mod wallpaper;
mod longclip;
#[cfg(windows)]
mod mp4;

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

// ---------- catalog ----------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaCatalog {
    pub engine: EngineSpec,
    pub music_engine: EngineSpec,
    pub models: Vec<MediaModel>,
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
        "generate" | "edit" | "fill" | "extend" | "restyle" | "tune" | "coach" => Kind::Image,
        "upscale" => Kind::Upscale,
        "remove_background" => Kind::Background,
        "video" => Kind::Video,
        "music" | "sound" | "parts" => Kind::Music,
        _ => return None,
    })
}

/// What a job asks of its model: a song's parts (adding, taking out or
/// completing one) need ACE-Step Base; everything else is the job's own kind.
fn model_op(req: &Request) -> &str {
    match req.music.as_ref().and_then(|m| m.task.as_deref()) {
        Some("add" | "extract" | "complete") if req.op == "music" => "parts",
        _ => &req.op,
    }
}

fn op_feature(op: &str) -> Option<Feature> {
    match op {
        "narrate" => Some(Feature::Music),
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
    let gpu_ok = b.vram > 0 && b.vram as f64 >= m.min_vram_gb * GIB as f64;
    // Weights that don't fit on the card are streamed from memory.
    let mem_ok = m.size() <= b.vram + b.ram;
    let (runnable, on_gpu, est, reason) = if !mem_ok {
        (false, false, 0.0, Some(format!("needs about {:.0} GB of memory", m.size() as f64 / GIB as f64 + 1.0)))
    } else {
        match m.kind {
            // These engines run on the processor.
            Kind::Music | Kind::Background => (true, false, m.secs * cpu_scale, None),
            _ if gpu_ok => (true, true, m.secs * REF_GPU_BW / b.gpu_bw_gbs.max(50.0), None),
            _ if m.cpu => (true, false, m.secs * CPU_SLOWDOWN * cpu_scale, None),
            _ => {
                let have = b.vram as f64 / GIB as f64;
                let why = if b.vram == 0 {
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

fn sd_dir(c: &MediaCatalog, backend: &str) -> String {
    format!("sd-{}-{backend}", c.engine.build)
}

fn music_dir(c: &MediaCatalog, key: &str) -> String {
    format!("acestep-{}-{key}", c.music_engine.build)
}

/// The installed music engine: the graphics-card build if it's here, else
/// the processor one.
fn music_engine_dir(state: &AppState) -> Option<PathBuf> {
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
            && (!needs_image_input || kind != Kind::Video || m.can("image"))
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
        b.vram > 0 && b.vram as f64 >= m.min_vram_gb * GIB as f64
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
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
}

impl MusicSpec {
    /// Says enough about the song to make one without other words.
    pub fn describes(&self) -> bool {
        self.genre.as_deref().is_some_and(|g| !g.trim().is_empty()) || !self.instruments.is_empty()
    }

    /// The music model's description: the person's words, then the style,
    /// mood, instruments, voice and beat as ACE-Step's captions put them.
    pub fn caption(&self, words: &str, instrumental: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        let words = words.trim().trim_end_matches('.');
        if !words.is_empty() {
            parts.push(words.to_string());
        }
        let some = |o: &Option<String>| o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(g) = some(&self.genre) {
            parts.push(g.to_lowercase());
        }
        if let Some(m) = some(&self.mood) {
            parts.push(format!("{} mood", m.to_lowercase()));
        }
        let inst: Vec<String> = self.instruments.iter().map(|i| i.trim().to_lowercase()).filter(|i| !i.is_empty()).collect();
        if !inst.is_empty() {
            parts.push(inst.join(", "));
        }
        if instrumental {
            parts.push("instrumental, no vocals".into());
        } else if let Some(v) = some(&self.voice) {
            let v = v.to_lowercase();
            parts.push(match v.as_str() {
                "female" | "male" | "choir" | "rap" => format!("{v} vocals"),
                "duet" => "male and female duet vocals".into(),
                other => format!("{other} vocals"),
            });
        }
        if let Some(b) = some(&self.beat) {
            parts.push(format!("{} beat", b.to_lowercase()));
        }
        parts.join(", ")
    }
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
    /// Music: how the song is shaped, and work on a song the person has.
    pub music: Option<MusicSpec>,
    pub voice: Option<String>,
    pub language: Option<String>,
    pub chat_id: Option<String>,
    /// How a new picture looks (see `STYLES`); none or "auto": a realistic
    /// photo, unless the description itself names a look.
    pub style: Option<String>,
    /// Example pictures (gallery ids) the new one should follow.
    pub examples: Vec<String>,
    /// What to take from them: "style", "subject" or "layout".
    pub example_use: Option<String>,
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

/// One job at a time: they share the graphics card and memory.
fn turn() -> &'static tokio::sync::Mutex<()> {
    static T: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    T.get_or_init(Default::default)
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

pub fn check(state: &AppState, req: &Request) -> Result<(), String> {
    let f = op_feature(&req.op).ok_or("Unknown kind of job.")?;
    crate::features::require(state, f)?;
    if let Some(id) = cloud_pick(state, req) {
        let c = crate::cloud::media_choices(&state.db.lock().unwrap()).into_iter().find(|c| c.id == id);
        match c {
            Some(c) if !cloud_can(&c, &req.op) => return Err(format!("{} can't do that kind of job.", c.name)),
            None => return Err("That cloud model isn't available: it needs the Cloud level, the provider turned on and its key.".into()),
            _ => {}
        }
    } else if req.op != "narrate" {
        choose(state, model_op(req), req.model_id.as_deref(), req.source.is_some())?;
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
    check(state, &req)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let on_cloud = cloud_pick(state, &req);
    let model = if req.op == "narrate" || on_cloud.is_some() { None } else { choose(state, model_op(&req), req.model_id.as_deref(), req.source.is_some()).ok() };
    let est = match (&model, &on_cloud) {
        (Some(m), _) => estimate(state, m, &req),
        (None, Some(_)) if req.op == "video" => 180.0,
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

    // Waiting for the job before it; removing it from the queue ends the wait
    // (a chat's Stop hung until the job before finished).
    let _turn = {
        let lock = turn().lock();
        tokio::pin!(lock);
        loop {
            tokio::select! {
                g = &mut lock => break g,
                _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(download::CANCELLED.into());
                    }
                }
            }
        }
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
    let card_engine = matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "video" | "tune" | "coach")
        || (matches!(req.op.as_str(), "music" | "sound") && music_engine_dir(state).is_some_and(|d| d.to_string_lossy().contains("vulkan")));
    let (gpu_name, cpu_name) = {
        let hw = state.hardware.read().unwrap();
        (hw.best_gpu().map(|g| g.name.clone()), hw.cpu_name.clone())
    };
    let has_card = state.budget().vram > 0 && gpu_name.is_some();
    let on_processor = Arc::new(AtomicBool::new(!(card_engine && has_card)));
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
                let (on, device) = device_now();
                r.lock().unwrap().push(Reading { gpu_usage: g.usage, gpu_temp: g.temp, cpu_usage: cpu, on_card: on == "graphics card" });
                update(&st, &id2, |i| {
                    i.gpu_usage = g.usage;
                    i.gpu_temp = g.temp;
                    i.cpu_usage = cpu;
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
        None => crate::vision::JOB_STOP.scope(cancel.clone(), run_job(state, &id, &req, model.as_ref(), &cancel, &progress)).await,
    };
    if let (Ok(items), Some(m)) = (&result, &model) {
        if !items.is_empty() {
            learn_speed(state, m, &req, started.elapsed().as_secs_f64());
            crate::cleanup::touch_model(state, &m.id);
        }
    }
    let name = match req.op.as_str() {
        "video" => "a video",
        "music" => "a song",
        "sound" => "a sound",
        "narrate" => "a narration",
        _ => "a picture",
    };
    watching.store(false, Ordering::Relaxed);
    let took = started.elapsed().as_secs_f64();
    let with = model.as_ref().map(|m| m.id.clone()).or_else(|| on_cloud.clone()).unwrap_or_else(|| "the built-in voice".into());
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
    let mut out = if flag_value(args, "--max-vram").is_some() { args.to_vec() } else { with_flag(args, "--max-vram", Some("-1")) };
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

/// After the card failed: more of it left free (or half the budget), the
/// text reader on the processor, and no fast attention. With the card
/// `pinned` (the only one the engine sees), the rest is named for it, since
/// the engine's own placement otherwise may move it to integrated graphics.
fn lighter_on_card(args: &[std::ffi::OsString], pinned: bool) -> Vec<std::ffi::OsString> {
    let budget = flag_value(args, "--max-vram").and_then(|v| v.parse::<f64>().ok()).unwrap_or(-1.0);
    let lighter = if budget > 0.0 { format!("{:.1}", (budget / 2.0).max(1.0)) } else { "-3".to_string() };
    let mut out: Vec<std::ffi::OsString> = with_flag(args, "--max-vram", Some(&lighter)).into_iter().filter(|a| a != "--diffusion-fa").collect();
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
        let vram = l.vram_bytes.min(b.vram).min(b.vram.saturating_sub(self.reserved));
        sd::Limits {
            threads: l.threads,
            // Only cap it when the performance mode or a loaded chat model leaves less than the card has.
            max_vram_gib: (b.vram > 0 && vram < b.vram).then(|| vram as f64 / GIB as f64),
        }
    }

    fn files(&self, m: &MediaModel) -> Vec<(String, PathBuf)> {
        m.files.iter().map(|f| (f.role.clone(), file_path(&self.state.paths, m, f))).collect()
    }

    async fn run_sd(&self, args: Vec<std::ffi::OsString>, work: &std::path::Path) -> Result<(), String> {
        let vram_gib = self.state.budget().vram as f64 / GIB as f64;
        let on_card = vram_gib > 0.0;
        if !on_card {
            return self.run_sd_once(args, work, None).await;
        }
        // Kept on the dedicated card: on a PC with integrated graphics too, the
        // engine would otherwise use whichever card it lists first.
        let pinned = crate::engine::learn_devices(self.state).await;
        // Planned to stay on the card: headroom it keeps free, and the
        // picture decoded in tiles where the last step's burst could overflow it.
        let first = card_friendly(&args, vram_gib);
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
    let vram = state.budget().vram;
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
    match crate::llm_endpoint(state, Some(paused.model_id.clone()), paused.vision, || {}).await {
        Ok(_) => crate::applog::line("media", &format!("Started {} again after the picture", paused.model_id)),
        Err(e) => crate::applog::line("media", &format!("Couldn't start {} again after the picture: {e}", paused.model_id)),
    }
}

async fn run_job(state: &Arc<AppState>, id: &str, req: &Request, model: Option<&MediaModel>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let music_on_gpu = matches!(req.op.as_str(), "music" | "sound") && music_engine_dir(state).is_some_and(|d| d.to_string_lossy().contains("vulkan"));
    // Lyrics come from the chat model: written before the card is cleared for
    // the music engine, not after (that loaded the chat model again beside it).
    let mut with_lyrics;
    let req = if wants_written_lyrics(req) {
        progress("Writing lyrics", 0.02);
        let seconds = req.seconds.unwrap_or(model.map_or(30.0, |m| m.defaults.seconds.max(30.0))).clamp(10.0, 600.0);
        with_lyrics = req.clone();
        // Told the style, mood and voice too, not only the person's words.
        match lyrics::write(state, &song_caption(req), seconds, req.language.as_deref()).await {
            Ok(l) => with_lyrics.lyrics = Some(l),
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
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    // Not for upscaling: its model is 64 MB, and stopping a 14 GB chat model
    // for it cost the next message a reload.
    let paused = if music_on_gpu || matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "video" | "tune" | "coach") {
        make_room(state, req, model).await
    } else {
        None
    };
    let result = match model.filter(|m| m.kind != Kind::Upscale) {
        Some(m) if matches!(req.op.as_str(), "generate" | "video") => made_well(state, id, req, m, cancel, progress).await,
        // Edits are planned and checked unless that's turned off, when something here reads pictures.
        Some(m) if req.op == "edit" && req.check != Some(false) && (crate::vision::can_read(state, None) || installed_file(state, m, "llm_vision").is_some()) => {
            edited_well(state, id, req, m, cancel, progress).await
        }
        _ => run_kind(state, id, req, model, cancel, progress).await,
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
    let vram = state.budget().vram;
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
        "music" | "sound" => song(&ctx, model.ok_or("No music model.")?, &work.0).await,
        "narrate" => narrate(&ctx).await,
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
            let mut prompt = styled(&req.prompt, req.style.as_deref());
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
            // (his brother's pictures lost what he wanted within a few edits).
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
        .filter(|n| !n.is_empty());
    let args = sd::picture_args(
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
    let can_continue = m.can("image") && cfg!(windows) && req.wallpaper.is_none();
    let most = if can_continue { longclip::MAX_SECS } else { longest };
    let seconds = req.seconds.unwrap_or(if req.wallpaper.is_some() { longest } else { d.seconds }).clamp(1.0, most.max(1.0));
    let total = (seconds * fps as f32).round() as u32 + 1;
    let parts = if can_continue { longclip::plan(total, max_frames) } else { vec![sd::frames_for(seconds, fps).min(max_frames)] };
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
    let bytes = std::fs::read(&out).map_err(|_| "The video engine didn't produce a clip.".to_string())?;
    let item = MediaItem {
        kind: "video".into(),
        op: "video".into(),
        prompt: req.asked.as_deref().unwrap_or(&req.prompt).trim().to_string(),
        model_id: Some(m.id.clone()),
        mime: "video/webm".into(),
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
        match lyrics::write(ctx.state, &caption_words, length, req.language.as_deref()).await {
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
    // The planning model isn't used (see `music::make`); older installs still have it.
    let dit = m.file("dit").ok_or("The music model is missing a file.")?;
    let lm_file = m.file("lm").map(|f| f.file.clone()).unwrap_or_default();
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
    let lyrics_text = if task.is_some() && words == lyrics::Words::MusicModelWrites { String::new() } else { words.for_engine().to_string() };
    let engine_dir = music_engine_dir(ctx.state).ok_or("The music engine isn't installed. Reinstall the music model from the Studio's Models list.")?;
    let models = model_dir(&ctx.state.paths, m);
    let versions = if task.is_none() && !sound { req.count.unwrap_or(1).clamp(1, 3) } else { 1 };
    let made = music::make(
        &music::Engine { dir: &engine_dir, models: &models, low_priority: ctx.state.limits().low_priority, job: ctx.state.job(), logs: &ctx.state.paths.logs },
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
            reference: reference.as_ref().map(|r| r.1.as_path()),
            versions,
        },
        ctx.cancel,
        ctx.progress,
    )
    .await?;
    let planned = made[0].lyrics.clone();
    // Vocals were asked for, and the music model planned nothing to sing:
    // say so rather than hand back an instrumental as if it were the song.
    if task.is_none() {
        if let Err(e) = lyrics::check_sung(&words, &planned, problem.as_ref()) {
            crate::applog::line("media", &format!("The song came out without words to sing (planned lyrics: {:?})", planned.chars().take(80).collect::<String>()));
            return Err(e);
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
            lyrics: (!sound && lyrics::has_singing(&song.lyrics)).then_some(song.lyrics),
            ..Default::default()
        };
        items.push(store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &song.mp3, None)?);
    }
    Ok(items)
}

async fn narrate(ctx: &RunCtx<'_>) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let text = crate::tts::speakable(req.prompt.trim());
    if text.trim().is_empty() {
        return Err("Type the text to read aloud.".into());
    }
    let (voice, language) = (req.voice.clone(), req.language.clone().filter(|l| l != "auto"));
    (ctx.progress)("Reading", 0.3);
    let speech = tokio::task::spawn_blocking(move || crate::tts::synthesize(&text, voice.as_deref(), language.as_deref(), 1.0))
        .await
        .map_err(|e| e.to_string())??;
    let floats: Vec<f32> = speech.samples.iter().map(|s| *s as f32 / 32768.0).collect();
    let wav = crate::audio::wav_encode(&floats, speech.rate);
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "audio".into(),
        op: "narrate".into(),
        prompt: req.prompt.trim().to_string(),
        mime: "audio/wav".into(),
        seconds: floats.len() as f64 / speech.rate.max(1) as f64,
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

#[tauri::command(async)]
pub fn media_view(state: AppStateRef) -> MediaView {
    view(&state)
}

fn view(state: &AppState) -> MediaView {
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
            continue;
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
pub fn install_media_model(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
    let spec = models(&state).into_iter().find(|m| m.id == model_id).ok_or("Unknown model")?;
    start_install(app, state.inner().clone(), spec, None)
}

/// The Studio's Hugging Face search: picture, video or music models built
/// on a family SulcusAI runs.
#[tauri::command]
pub async fn media_hub_search(state: AppStateRef<'_>, kind: String, query: String, sort: String, all: Option<bool>) -> Result<hub::HubMediaResults, String> {
    hub::search(state.inner(), &kind, &query, &sort, all.unwrap_or(false)).await
}

/// Installs one file of a Hugging Face repository like a catalog model.
#[tauri::command]
pub async fn media_hub_install(app: AppHandle, state: AppStateRef<'_>, repo: String, file: String) -> Result<String, String> {
    let spec = hub::prepare(state.inner(), &repo, &file).await?;
    let id = spec.id.clone();
    if let Err(e) = start_install(app, state.inner().clone(), spec, None) {
        hub::forget(&state.db.lock().unwrap(), &id).ok();
        return Err(e);
    }
    Ok(id)
}

/// Downloads a model in the background; `then_feature` is turned on after.
pub(crate) fn start_install(app: AppHandle, state: Arc<AppState>, spec: MediaModel, then_feature: Option<Feature>) -> Result<(), String> {
    let hw = state.hardware.read().unwrap().clone();
    let f = fit(&spec, &hw, &Budget::from_hardware(&hw), missing_bytes(&state.paths, &spec), None);
    if !f.runnable {
        return Err(format!("This PC can't run {}: it {}.", spec.name, f.reason.unwrap_or_else(|| "needs more".into())));
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
    check(&state, &request)?;
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
    let (ep, _, _) = crate::llm_endpoint(state.inner(), None, false, || {}).await?;
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
    state.cipher()?;
    let conn = state.db.lock().unwrap();
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

/// Lyrics for a song, written before it's made so the person can read and
/// change them (the Studio's "Write lyrics for me").
#[tauri::command]
pub async fn music_write_lyrics(state: AppStateRef<'_>, prompt: String, music: Option<MusicSpec>, seconds: Option<f32>, language: Option<String>) -> Result<String, String> {
    state.cipher()?;
    let req = Request { op: "music".into(), prompt, music, ..Default::default() };
    let words = song_caption(&req);
    if words.trim().is_empty() {
        return Err("Describe the song first (or pick its style), so the lyrics fit it.".into());
    }
    lyrics::write(state.inner(), &words, seconds.unwrap_or(60.0).clamp(10.0, 600.0), language.as_deref().filter(|l| *l != "auto"))
        .await
        .map_err(|why| match why {
            lyrics::NotWritten::NoModel(_) => "There's no chat model on this PC to write lyrics. Install one from Models, or type your own.".to_string(),
            lyrics::NotWritten::Unusable(e) => format!("The lyrics didn't come out right ({e}). Try again, or type your own."),
        })
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
    let op = if matches!(op.as_str(), "trim" | "paste" | "attach" | "screenshot" | "import") { op } else { "import".into() };
    let mut item = add(&state, &c, bytes, kind, &mime, &op, prompt.as_deref().unwrap_or(""), parent, chat_id, hidden.unwrap_or(false))?;
    if let Some(s) = seconds {
        item.seconds = s;
        store::update(&state.db.lock().unwrap(), &c, &item)?;
    }
    Ok(item)
}

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

    /// The Studio's choices become the music model's description; a part
    /// job asks for the model that does parts; lyrics are written only for
    /// songs that sing.
    #[test]
    fn a_shaped_song_is_described_and_routed() {
        let spec = MusicSpec { genre: Some("Lo-fi hip hop".into()), mood: Some("Calm".into()), instruments: vec!["Piano".into(), "Vinyl crackle".into()], voice: Some("female".into()), beat: Some("Boom bap".into()), ..Default::default() };
        assert_eq!(spec.caption("rainy evening study session.", false), "rainy evening study session, lo-fi hip hop, calm mood, piano, vinyl crackle, female vocals, boom bap beat");
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
            std::thread::spawn(move || tx.send(view(&s).gallery).ok());
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

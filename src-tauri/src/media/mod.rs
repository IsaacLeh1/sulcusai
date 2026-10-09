// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures, video, music and narration, made on this PC.
//!
//! stable-diffusion.cpp makes and edits pictures, upscales them and makes
//! video clips; acestep.cpp makes music; BiRefNet on ONNX Runtime removes
//! backgrounds; narration uses the app's own voices. Each job runs the
//! engine program once, within the performance limits, one job at a time,
//! and what it makes goes into the gallery, encrypted.

mod cutout;
mod imaging;
mod music;
mod proc;
pub mod screen;
mod sd;
pub mod store;

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

const SD_EXE: &str = "sd-cli.exe";
const MUSIC_EXE: &str = "ace-synth.exe";
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
        "generate" | "edit" | "fill" | "extend" | "restyle" => Kind::Image,
        "upscale" => Kind::Upscale,
        "remove_background" => Kind::Background,
        "video" => Kind::Video,
        "music" | "sound" => Kind::Music,
        _ => return None,
    })
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
}

const INSTALLED_KEY: &str = "media_installed";

pub fn installed(conn: &Connection) -> Vec<InstalledMedia> {
    db::get(conn, INSTALLED_KEY).unwrap_or_default()
}

fn save_installed(conn: &Connection, list: &[InstalledMedia]) -> Result<(), String> {
    db::set(conn, INSTALLED_KEY, &list)
}

/// Any model of this kind installed.
pub fn has_kind(state: &AppState, kind: Kind) -> bool {
    let list = installed(&state.db.lock().unwrap());
    state.catalog.media.models.iter().any(|m| m.kind == kind && list.iter().any(|i| i.model_id == m.id))
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

fn missing_bytes(paths: &Paths, m: &MediaModel) -> u64 {
    m.files.iter().filter(|f| !file_path(paths, m, f).metadata().is_ok_and(|md| md.len() == f.size)).map(|f| f.size).sum()
}

fn sd_dir(c: &MediaCatalog, backend: &str) -> String {
    format!("sd-{}-{backend}", c.engine.build)
}

fn music_dir(c: &MediaCatalog) -> String {
    format!("acestep-{}-cpu-x64", c.music_engine.build)
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
    let ok = |m: &&MediaModel| {
        m.kind == kind && list.iter().any(|i| i.model_id == m.id) && (kind != Kind::Image || m.can(op)) && (!needs_image_input || kind != Kind::Video || m.can("image"))
    };
    if let Some(id) = wanted.filter(|w| !w.is_empty()) {
        if let Some(m) = state.catalog.media.models.iter().find(|m| m.id == id).filter(ok) {
            return Ok(m.clone());
        }
    }
    if let Some(m) = state.catalog.media.models.iter().filter(ok).max_by_key(|m| m.quality) {
        return Ok(m.clone());
    }
    Err(match (kind, op) {
        (Kind::Image, "edit" | "restyle") => "Edits from instructions need FLUX.2 klein. Install it from the Studio's Models list.".into(),
        (Kind::Image, _) => "Install a picture model first: open the Studio and pick one under Models.".into(),
        (Kind::Video, _) if needs_image_input => "Bringing a picture to life needs Wan 2.2. Install it from the Studio's Models list.".into(),
        (Kind::Video, _) => "Install a video model first: open the Studio and pick one under Models.".into(),
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
            let asset = c.music_engine.assets.get("cpu-x64").ok_or("No music engine for this PC")?;
            let dir = music_dir(c);
            if engine::find_exe(&state.paths.engines.join(&dir), MUSIC_EXE).is_none() {
                state.log("network", &format!("Downloading the acestep.cpp music engine from {}", crate::host_of(&asset.url)));
            }
            engine::ensure_unpacked(&state.paths, &dir, asset, MUSIC_EXE, cancel, |r, t| emit("engine", r, t)).await?;
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
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    if !list.iter().any(|i| i.model_id == spec.id) {
        list.push(InstalledMedia { model_id: spec.id.clone(), installed_at: db::now_ms(), secs: None });
    }
    save_installed(&conn, &list)
}

#[cfg(test)]
pub async fn install_for_test(state: &Arc<AppState>, id: &str) -> Result<(), String> {
    let spec = state.catalog.media.models.iter().find(|m| m.id == id).cloned().ok_or("unknown")?;
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
    pub voice: Option<String>,
    pub language: Option<String>,
    pub chat_id: Option<String>,
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
}

struct Job {
    info: JobInfo,
    cancel: Arc<AtomicBool>,
}

fn jobs() -> &'static Mutex<Vec<Job>> {
    static J: OnceLock<Mutex<Vec<Job>>> = OnceLock::new();
    J.get_or_init(Default::default)
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
pub fn check(state: &AppState, req: &Request) -> Result<(), String> {
    let f = op_feature(&req.op).ok_or("Unknown kind of job.")?;
    crate::features::require(state, f)?;
    if req.op != "narrate" {
        choose(state, &req.op, req.model_id.as_deref(), req.source.is_some())?;
    }
    let needs_source = matches!(req.op.as_str(), "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background");
    if needs_source && req.source.is_none() {
        return Err("Pick a picture to work on first.".into());
    }
    let needs_prompt = matches!(req.op.as_str(), "generate" | "edit" | "restyle" | "video" | "music" | "sound" | "narrate");
    if needs_prompt && req.prompt.trim().is_empty() && !(req.op == "video" && req.source.is_some()) {
        return Err(if req.op == "narrate" { "Type the text to read aloud." } else { "Describe what you'd like first." }.into());
    }
    if req.op == "fill" && req.mask.is_none() {
        return Err("Paint over the part to change first.".into());
    }
    Ok(())
}

/// Queues a job and runs it when it's its turn. Returns what it made.
pub async fn run(state: &Arc<AppState>, id: String, req: Request) -> Result<Vec<MediaItem>, String> {
    check(state, &req)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let model = if req.op == "narrate" { None } else { choose(state, &req.op, req.model_id.as_deref(), req.source.is_some()).ok() };
    let est = model.as_ref().map_or(10.0, |m| estimate(state, m, &req));
    {
        let info = JobInfo {
            id: id.clone(),
            op: req.op.clone(),
            prompt: req.prompt.chars().take(200).collect(),
            model_id: model.as_ref().map(|m| m.id.clone()),
            status: "queued".into(),
            stage: "Waiting for the job before it".into(),
            fraction: 0.0,
            eta_secs: None,
            est_secs: est,
            chat_id: req.chat_id.clone(),
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

    let _turn = turn().lock().await;
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let started = Instant::now();
    update(state, &id, |i| {
        i.status = "running".into();
        i.stage = "Starting".into();
    });
    let progress = |stage: &str, fraction: f64| {
        let elapsed = started.elapsed().as_secs_f64();
        let eta = if fraction > 0.12 { elapsed / fraction - elapsed } else { (est - elapsed).max(5.0) };
        update(state, &id, |i| {
            i.stage = stage.to_string();
            i.fraction = fraction.clamp(0.0, 1.0);
            i.eta_secs = Some(eta.max(0.0).round());
        });
    };
    let result = run_job(state, &id, &req, model.as_ref(), &cancel, &progress).await;
    if let (Ok(items), Some(m)) = (&result, &model) {
        if !items.is_empty() {
            learn_speed(state, m, &req, started.elapsed().as_secs_f64());
        }
    }
    let name = match req.op.as_str() {
        "video" => "a video",
        "music" => "a song",
        "sound" => "a sound",
        "narrate" => "a narration",
        _ => "a picture",
    };
    match &result {
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
        Kind::Music => (req.seconds.unwrap_or(m.defaults.seconds).max(10.0) / m.defaults.seconds.max(10.0)) as f64,
        Kind::Image => req.count.unwrap_or(1).clamp(1, 4) as f64,
        _ => 1.0,
    }
}

fn estimate(state: &AppState, m: &MediaModel, req: &Request) -> f64 {
    let hw = state.hardware.read().unwrap().clone();
    let measured = installed(&state.db.lock().unwrap()).into_iter().find(|i| i.model_id == m.id).and_then(|i| i.secs);
    let f = fit(m, &hw, &state.budget(), 0, measured);
    (f.est_secs * job_scale(m, req)).max(3.0)
}

/// Keeps a running average of how long a default job takes here.
fn learn_speed(state: &AppState, m: &MediaModel, req: &Request, secs: f64) {
    let per = secs / job_scale(m, req).max(0.1);
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    if let Some(i) = list.iter_mut().find(|i| i.model_id == m.id) {
        i.secs = Some(match i.secs {
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
}

impl RunCtx<'_> {
    fn limits(&self) -> sd::Limits {
        let l = self.state.limits();
        let b = self.state.budget();
        let vram = l.vram_bytes.min(b.vram);
        sd::Limits {
            threads: l.threads,
            // Only cap it when the performance mode asks for less than the card has.
            max_vram_gib: (b.vram > 0 && vram < b.vram).then(|| vram as f64 / GIB as f64),
        }
    }

    fn files(&self, m: &MediaModel) -> Vec<(String, PathBuf)> {
        m.files.iter().map(|f| (f.role.clone(), file_path(&self.state.paths, m, f))).collect()
    }

    async fn run_sd(&self, args: Vec<std::ffi::OsString>, work: &std::path::Path) -> Result<(), String> {
        let exe = sd_exe(self.state)?;
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

    fn source(&self) -> Result<(MediaItem, image::DynamicImage), String> {
        let id = self.req.source.as_deref().ok_or("Pick a picture to work on first.")?;
        let c = self.state.work_cipher()?;
        let item = store::get(&self.state.db.lock().unwrap(), &c, id).ok_or("That picture is no longer in the gallery.")?;
        if item.kind != "image" {
            return Err("That isn't a picture.".into());
        }
        let bytes = store::read(&self.state.paths, &c, id)?;
        Ok((item, imaging::decode(&bytes)?))
    }

    /// Saves a made picture into the gallery.
    fn keep_picture(&self, img: &image::DynamicImage, m: Option<&MediaModel>, parent: Option<&str>, seed: Option<i64>) -> Result<MediaItem, String> {
        let c = self.state.work_cipher()?;
        let png = imaging::png(img)?;
        let thumb = imaging::thumbnail(img)?;
        let (w, h) = img.dimensions();
        let item = MediaItem {
            kind: "image".into(),
            op: self.req.op.clone(),
            prompt: self.req.prompt.trim().to_string(),
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
async fn make_room(state: &Arc<AppState>) {
    if state.budget().vram > 0 && state.generations.lock().unwrap().is_empty() {
        let mut e = state.engine.lock().await;
        if e.status().model_id.is_some() {
            e.stop().await;
            emit(state, "engine:unloaded", json!({}));
        }
    }
}

async fn run_job(state: &Arc<AppState>, id: &str, req: &Request, model: Option<&MediaModel>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let ctx = RunCtx { state, req, cancel, progress };
    let work = work_dir(&state.paths, id)?;
    match req.op.as_str() {
        "generate" | "edit" | "fill" | "extend" | "restyle" => {
            make_room(state).await;
            picture(&ctx, model.ok_or("No picture model.")?, &work.0).await
        }
        "upscale" => {
            make_room(state).await;
            upscale(&ctx, model.ok_or("No upscaler.")?, &work.0).await
        }
        "remove_background" => remove_background(&ctx, model.ok_or("No background remover.")?).await,
        "video" => {
            make_room(state).await;
            video(&ctx, model.ok_or("No video model.")?, &work.0).await
        }
        "music" | "sound" => song(&ctx, model.ok_or("No music model.")?, &work.0).await,
        "narrate" => narrate(&ctx).await,
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
    let mut source: Option<(MediaItem, image::DynamicImage)> = None;
    let mut fill_mask: Option<image::GrayImage> = None;
    let mut count = 1;
    let (prompt, w, h, init, mask, refs) = match req.op.as_str() {
        "generate" => {
            count = req.count.unwrap_or(1).clamp(1, 4);
            let (w, h) = match (req.width, req.height) {
                (Some(w), Some(h)) => (imaging::round16(w.clamp(256, 2048) as f64), imaging::round16(h.clamp(256, 2048) as f64)),
                _ => imaging::shape_size(req.shape.as_deref().unwrap_or("square"), side),
            };
            (req.prompt.trim().to_string(), w, h, None, None, vec![])
        }
        "edit" | "restyle" => {
            let s = ctx.source()?;
            let (w, h) = imaging::work_size(s.1.width(), s.1.height(), side * side);
            let src = work.join("source.png");
            std::fs::write(&src, imaging::png(&s.1.resize_exact(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
            let prompt = if req.op == "restyle" {
                format!("Redraw this picture in this style: {}. Keep the same subject, layout and composition.", req.prompt.trim())
            } else {
                req.prompt.trim().to_string()
            };
            source = Some(s);
            (prompt, w, h, None, None, vec![src])
        }
        "fill" => {
            let s = ctx.source()?;
            let (w, h) = imaging::work_size(s.1.width(), s.1.height(), side * side);
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
            let (canvas, mk) = imaging::extend(&s.1, l, t, r, b);
            let (w, h) = imaging::work_size(canvas.width(), canvas.height(), side * side);
            let canvas = image::DynamicImage::ImageRgba8(canvas).resize_exact(w, h, image::imageops::FilterType::Lanczos3);
            let mk = image::imageops::resize(&mk, w, h, image::imageops::FilterType::Nearest);
            let (src, mpath) = (work.join("source.png"), work.join("mask.png"));
            std::fs::write(&src, imaging::png(&canvas)?).map_err(|e| e.to_string())?;
            std::fs::write(&mpath, imaging::png(&image::DynamicImage::ImageLuma8(mk))?).map_err(|e| e.to_string())?;
            let prompt = if req.prompt.trim().is_empty() {
                "Continue the picture naturally beyond its edges, with the same style, lighting and detail.".to_string()
            } else {
                req.prompt.trim().to_string()
            };
            source = Some(s);
            (prompt, w, h, Some(src), Some(mpath), vec![])
        }
        _ => return Err("Unknown kind of picture job.".into()),
    };
    let args = sd::picture_args(
        &ctx.files(m),
        d,
        &sd::Picture {
            prompt: &prompt,
            negative: req.negative.as_deref(),
            width: w,
            height: h,
            steps,
            seed,
            count,
            init: init.as_deref(),
            mask: mask.as_deref(),
            refs: refs.iter().map(|p| p.as_path()).collect(),
            strength: init.as_ref().map(|_| 1.0),
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
        }
        items.push(ctx.keep_picture(&img, Some(m), source.as_ref().map(|s| s.0.id.as_str()), Some(seed + i as i64))?);
    }
    Ok(items)
}

async fn upscale(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let (item, img) = ctx.source()?;
    // 4x of a big picture is huge (and slow); start from at most 1024 px,
    // which comes out at about 4096.
    let img = imaging::cap(img, 1024);
    let (src, out) = (work.join("source.png"), work.join("out.png"));
    std::fs::write(&src, imaging::png(&img)?).map_err(|e| e.to_string())?;
    let args = sd::upscale_args(&ctx.files(m), &src, &out, &ctx.limits());
    ctx.run_sd(args, work).await?;
    let big = imaging::cap(imaging::decode(&std::fs::read(&out).map_err(|_| "The upscaler didn't produce a picture.".to_string())?)?, 4096);
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
    let seconds = req.seconds.unwrap_or(d.seconds).clamp(1.0, 8.0);
    let frames = sd::frames_for(seconds, fps);
    let (mut w, mut h) = (d.width.max(256), d.height.max(256));
    if matches!(req.shape.as_deref(), Some("portrait" | "tall")) {
        std::mem::swap(&mut w, &mut h);
    } else if req.shape.as_deref() == Some("square") {
        let s = imaging::round16(((w * h) as f64).sqrt());
        (w, h) = (s, s);
    }
    let mut parent = None;
    let init = match req.source.as_deref() {
        Some(_) => {
            let (item, img) = ctx.source()?;
            // Same shape as the picture, at the model's size.
            let (iw, ih) = img.dimensions();
            let area = (w * h) as f64;
            let k = (area / (iw as f64 * ih as f64)).sqrt();
            (w, h) = (imaging::round16(iw as f64 * k), imaging::round16(ih as f64 * k));
            let p = work.join("start.png");
            std::fs::write(&p, imaging::png(&img.resize_exact(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
            parent = Some(item.id);
            Some(p)
        }
        None => None,
    };
    let prompt = if req.prompt.trim().is_empty() { "Gentle natural motion, the scene comes to life.".to_string() } else { req.prompt.trim().to_string() };
    let seed = req.seed.unwrap_or_else(random_seed);
    let out = work.join("out.webm");
    let args = sd::clip_args(
        &ctx.files(m),
        d,
        &sd::Clip { prompt: &prompt, width: w, height: h, frames, fps, steps: req.steps.unwrap_or(d.steps).clamp(4, 50), seed, init: init.as_deref(), out: &out },
        &ctx.limits(),
    );
    ctx.run_sd(args, work).await?;
    let bytes = std::fs::read(&out).map_err(|_| "The video engine didn't produce a clip.".to_string())?;
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "video".into(),
        op: "video".into(),
        prompt: req.prompt.trim().to_string(),
        model_id: Some(m.id.clone()),
        mime: "video/webm".into(),
        width: w,
        height: h,
        seconds: frames as f64 / fps as f64,
        parent,
        chat_id: req.chat_id.clone(),
        seed: Some(seed),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &bytes, None)?])
}

/// Lyrics from the chat model, so a song with vocals has real words to sing.
async fn write_lyrics(state: &Arc<AppState>, prompt: &str, seconds: f32, language: Option<&str>) -> Option<String> {
    let (ep, _) = crate::background_endpoint(state).await.ok()?;
    let lines = ((seconds / 30.0) * 8.0).round().clamp(6.0, 40.0) as u32;
    let lang = language.filter(|l| !l.is_empty() && *l != "auto").map(|l| format!(" Write them in the language with code \"{l}\"."));
    let messages = vec![
        json!({ "role": "system", "content": format!(
            "You write song lyrics. Reply with the lyrics only: about {lines} short lines, with section tags on their own lines such as [Verse], [Chorus] and [Bridge]. No title, no notes, no quotes.{}",
            lang.unwrap_or_default()
        ) }),
        json!({ "role": "user", "content": prompt }),
    ];
    let raw = crate::chat::complete(&ep, messages, json!({ "temperature": 0.8 }), 600).await.ok()?;
    let text = raw.trim().trim_matches('`').trim().to_string();
    (text.lines().filter(|l| !l.trim().is_empty()).count() >= 3).then_some(text)
}

async fn song(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let sound = req.op == "sound";
    let seconds = if sound { req.seconds.unwrap_or(10.0).clamp(10.0, 30.0) } else { req.seconds.unwrap_or(m.defaults.seconds.max(30.0)).clamp(10.0, 240.0) };
    let lyrics = if sound || req.instrumental == Some(true) {
        "[Instrumental]".to_string()
    } else if let Some(l) = req.lyrics.as_ref().filter(|l| !l.trim().is_empty()) {
        l.trim().to_string()
    } else {
        (ctx.progress)("Writing lyrics", 0.02);
        write_lyrics(ctx.state, req.prompt.trim(), seconds, req.language.as_deref()).await.unwrap_or_default()
    };
    let caption = if sound {
        format!("{}. A sound effect recording, not music: no melody, no singing.", req.prompt.trim().trim_end_matches('.'))
    } else {
        req.prompt.trim().to_string()
    };
    let (lm, dit) = (m.file("lm").ok_or("The music model is missing a file.")?, m.file("dit").ok_or("The music model is missing a file.")?);
    let seed = req.seed.unwrap_or_else(random_seed);
    // Left to itself with no lyrics or language, the music model can sing in
    // made-up words; English is the safer guess.
    let language = req.language.clone().filter(|l| !l.is_empty() && l != "auto").or_else(|| (!sound && lyrics.is_empty()).then(|| "en".to_string()));
    let engine_dir = ctx.state.paths.engines.join(music_dir(&ctx.state.catalog.media));
    let models = model_dir(&ctx.state.paths, m);
    let made = music::make(
        &music::Engine { dir: &engine_dir, models: &models, low_priority: ctx.state.limits().low_priority, job: ctx.state.job(), logs: &ctx.state.paths.logs },
        work,
        &music::SongRequest { caption: &caption, lyrics: &lyrics, seconds, seed, language: language.as_deref(), lm_file: &lm.file, dit_file: &dit.file },
        ctx.cancel,
        ctx.progress,
    )
    .await?;
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "audio".into(),
        op: req.op.clone(),
        prompt: req.prompt.trim().to_string(),
        model_id: Some(m.id.clone()),
        mime: "audio/mpeg".into(),
        seconds: made.seconds,
        chat_id: req.chat_id.clone(),
        seed: Some(seed),
        lyrics: (!sound && !made.lyrics.is_empty() && made.lyrics != "[Instrumental]").then_some(made.lyrics),
        ..Default::default()
    };
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &made.mp3, None)?])
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
    /// Models that can't run on this PC, left out of the list.
    hidden: usize,
    /// Why video isn't available here, when no video model fits.
    video_note: Option<String>,
    installing: Vec<String>,
    jobs: Vec<JobInfo>,
    gallery: usize,
}

#[tauri::command]
pub fn media_view(state: AppStateRef) -> MediaView {
    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let list = installed(&state.db.lock().unwrap());
    let mut hidden = 0;
    let mut video_note = None;
    let mut models = Vec::new();
    for m in &state.catalog.media.models {
        let inst = list.iter().find(|i| i.model_id == m.id);
        let f = fit(m, &hw, &b, missing_bytes(&state.paths, m), inst.and_then(|i| i.secs));
        if inst.is_none() && !(f.runnable && f.disk_ok) {
            hidden += 1;
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
    let c = state.cipher();
    MediaView {
        models,
        hidden,
        video_note,
        installing: state.installs.lock().unwrap().keys().cloned().collect(),
        jobs: list_jobs(),
        gallery: c.map(|_| store::count(&state.db.lock().unwrap())).unwrap_or(0),
    }
}

#[tauri::command]
pub fn install_media_model(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
    let spec = state.catalog.media.models.iter().find(|m| m.id == model_id).ok_or("Unknown model")?.clone();
    start_install(app, state.inner().clone(), spec, None)
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
pub async fn remove_media_model(state: AppStateRef<'_>, model_id: String) -> Result<(), String> {
    state.cipher()?;
    let spec = state.catalog.media.models.iter().find(|m| m.id == model_id).ok_or("Unknown model")?.clone();
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    list.retain(|i| i.model_id != model_id);
    // Keep files another installed model still uses.
    let keep: Vec<PathBuf> = state
        .catalog
        .media
        .models
        .iter()
        .filter(|m| list.iter().any(|i| i.model_id == m.id))
        .flat_map(|m| m.files.iter().map(|f| file_path(&state.paths, m, f)).collect::<Vec<_>>())
        .collect();
    for f in &spec.files {
        let p = file_path(&state.paths, &spec, f);
        if !keep.contains(&p) {
            std::fs::remove_file(&p).ok();
        }
    }
    if spec.kind == Kind::Background {
        cutout::unload();
    }
    save_installed(&conn, &list)?;
    db::log_action(&conn, "model", &format!("Removed {} and deleted its files", spec.name));
    Ok(())
}

#[tauri::command]
pub fn media_start(state: AppStateRef, request: Request) -> Result<String, String> {
    state.cipher()?;
    check(&state, &request)?;
    let id = store::new_id();
    let (state, job_id) = (state.inner().clone(), id.clone());
    tauri::async_runtime::spawn(async move {
        let result = run(&state, job_id.clone(), request).await;
        let payload = match result {
            Ok(items) => json!({ "job_id": job_id, "ok": true, "items": items }),
            Err(e) if e == download::CANCELLED => json!({ "job_id": job_id, "ok": false, "cancelled": true }),
            Err(e) => json!({ "job_id": job_id, "ok": false, "error": e }),
        };
        emit(&state, "media:done", payload);
    });
    Ok(id)
}

#[tauri::command]
pub fn media_cancel(job_id: String) {
    cancel(&job_id);
}

#[tauri::command]
pub fn media_jobs() -> Vec<JobInfo> {
    list_jobs()
}

#[tauri::command]
pub fn media_list(state: AppStateRef, kind: Option<String>, before: Option<i64>, limit: Option<usize>) -> Result<Vec<MediaItem>, String> {
    let c = state.cipher()?;
    Ok(store::list(&state.db.lock().unwrap(), &c, kind.as_deref(), limit.unwrap_or(60).min(500), before))
}

#[tauri::command]
pub fn media_get(state: AppStateRef, id: String) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    store::get(&state.db.lock().unwrap(), &c, &id).ok_or_else(|| "That item is no longer in the gallery.".into())
}

#[tauri::command]
pub fn media_file(state: AppStateRef, id: String) -> Result<tauri::ipc::Response, String> {
    let c = state.cipher()?;
    Ok(tauri::ipc::Response::new(store::read(&state.paths, &c, &id)?))
}

#[tauri::command]
pub fn media_thumb(state: AppStateRef, id: String) -> Result<tauri::ipc::Response, String> {
    let c = state.cipher()?;
    Ok(tauri::ipc::Response::new(store::read_thumb(&state.paths, &c, &id)?))
}

#[tauri::command]
pub fn media_delete(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    store::delete(&conn, &state.paths, &id)?;
    db::log_action(&conn, "media", "Deleted an item from the gallery");
    Ok(())
}

#[tauri::command]
pub fn media_favorite(state: AppStateRef, id: String, on: bool) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut item = store::get(&conn, &c, &id).ok_or("That item is no longer in the gallery.")?;
    item.favorite = on;
    store::update(&conn, &c, &item)?;
    Ok(item)
}

/// Saves a copy, unencrypted, where the user picked (Save as…).
#[tauri::command]
pub fn media_export(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    let c = state.cipher()?;
    let bytes = store::read(&state.paths, &c, &id)?;
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("media", "Saved a copy of a gallery item outside the app");
    Ok(())
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
        _ => return None,
    })
}

const MAX_IMPORT: u64 = 200 * 1024 * 1024;

/// Brings a picture, clip or sound from a file into the gallery.
#[tauri::command]
pub fn media_import(state: AppStateRef, path: String, chat_id: Option<String>) -> Result<MediaItem, String> {
    let c = state.cipher()?;
    let (kind, mime) = mime_for(&path).ok_or("Pick a picture (PNG, JPEG, WebP), a clip (WebM, MP4) or a sound (MP3, WAV, OGG).")?;
    // Attached to a chat: a hidden attachment, deleted with the chat.
    if chat_id.is_some() && kind != "image" {
        return Err("Only pictures can be attached to a chat.".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_IMPORT {
        return Err("That file is too big (200 MB at most).".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let hidden = chat_id.is_some();
    add(&state, &c, bytes, kind, mime, if hidden { "attach" } else { "import" }, &name, None, chat_id, hidden)
}

fn add(state: &AppState, c: &crate::crypto::Cipher, bytes: Vec<u8>, kind: &str, mime: &str, op: &str, prompt: &str, parent: Option<String>, chat_id: Option<String>, hidden: bool) -> Result<MediaItem, String> {
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
#[tauri::command]
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
                Kind::Music => assert!(m.file("lm").is_some() && m.file("dit").is_some()),
                Kind::Image | Kind::Video => assert!(m.file("diffusion").is_some() && m.file("vae").is_some()),
                _ => {}
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
    fn ops_map_to_features() {
        assert_eq!(op_feature("restyle"), Some(Feature::Images));
        assert_eq!(op_feature("remove_background"), Some(Feature::Images));
        assert_eq!(op_feature("video"), Some(Feature::Video));
        assert_eq!(op_feature("narrate"), Some(Feature::Music));
        assert_eq!(op_feature("nonsense"), None);
    }
}

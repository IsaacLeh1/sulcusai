// SPDX-License-Identifier: AGPL-3.0-only
//! Uncensored copies (Advanced mode): a copy of a chat model that answers
//! what the original refuses, made on this PC with Heretic (Philipp Emanuel
//! Weidmann, AGPL-3.0). Heretic finds the direction in the model's thinking
//! that makes it refuse, from requests it refuses and requests it answers
//! (mlabonne's harmful_behaviors and harmless_alpaca), then tries many ways
//! of taking that direction out of the weights. Each try is scored on how
//! many of 100 test requests it still refuses and how far its answers to
//! ordinary ones drift from the original's (KL divergence). SulcusAI's
//! runner (`scripts/heretic`) keeps the try that refuses least without
//! drifting past 0.5, Heretic's own mark for a noticeably damaged model.
//!
//! Heretic changes the model as its makers published it (safetensors, from
//! Hugging Face: a GGUF has lost too much detail to work on), so the
//! original is downloaded, changed on the graphics card with PyTorch,
//! turned into a GGUF by llama.cpp's converter at the engine's release, and
//! squeezed to the installed model's quantization by the engine's own
//! llama-quantize. The copy is a model of its own beside the original
//! ("<name> (uncensored)"), which is never changed. It changes what the
//! model will say, nothing else: the app's own checks (asking before
//! deleting, sending or sharing, the folders it may use, what may leave the
//! PC) apply to it as to any model.
//!
//! It runs on NVIDIA cards (PyTorch for CUDA), set up the first time by uv
//! beside the engines, sharing the music trainer's uv and package cache.
//! On an RTX 5070, Qwen2.5 0.5B took 4.5 s a try and refused 7 of 100 test
//! requests after 8 tries (the original: 91), drifting 0.066.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::catalog::{Asset, VisionFile};
use crate::db::{self, InstalledModel};
use crate::hub::{API, SITE};
use crate::media::proc::{self, Run};
use crate::media::train::{self, TrainerFile};
use crate::paths::Paths;
use crate::{engine, net, AppState, AppStateRef};

/// Heretic and what it needs, from the catalog.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HereticCatalog {
    pub name: String,
    pub license: String,
    /// Heretic's version and the llama.cpp release of the converter beside
    /// it ("1.4.0-b11450").
    pub build: String,
    /// uv, which sets up its Python and packages (the music trainer's).
    pub uv: Asset,
    /// SulcusAI's runner, the locked packages and llama.cpp's converter.
    pub source: Asset,
    /// Its prompt sets, by the name the runner reads them as
    /// ("harmful-train.parquet" and so on).
    pub prompts: Vec<TrainerFile>,
}

/// The most a kept try may drift from the original's answers (Heretic:
/// "KL divergence values above 0.5 usually indicate significant damage").
pub const MAX_KL: f64 = 0.5;
/// Tries Heretic makes: quick, and thorough (Heretic's own default).
pub const QUICK: u32 = 60;
pub const THOROUGH: u32 = 200;

/// Its runner, packages and converter.
pub fn source_dir(paths: &Paths, h: &HereticCatalog) -> PathBuf {
    paths.engines.join(format!("heretic-{}", h.build))
}

fn prompts_dir(paths: &Paths, h: &HereticCatalog) -> PathBuf {
    source_dir(paths, h).join("prompts")
}

/// Where a run keeps the original's download and what it makes from it,
/// until the copy is in: in the models folder, which has room for models.
fn work_root(paths: &Paths) -> PathBuf {
    paths.models.join("uncensor-work")
}

fn work_dir(paths: &Paths, repo: &str, revision: &str) -> PathBuf {
    work_root(paths).join(format!("{}-{}", repo.replace('/', "--"), &revision[..revision.len().min(7)]))
}

/// Whether it and its packages and prompts are set up.
pub fn ready(paths: &Paths, h: &HereticCatalog) -> bool {
    source_dir(paths, h).join(".venv").join("Scripts").join("python.exe").is_file() && h.prompts.iter().all(|f| prompts_dir(paths, h).join(&f.file).is_file())
}

/// How many bytes setting it up downloads, roughly (the packages' share is
/// an estimate, and nothing when the music trainer has PyTorch already).
pub fn setup_bytes(h: &HereticCatalog) -> u64 {
    h.uv.size + h.source.size + h.prompts.iter().map(|f| f.size).sum::<u64>() + 3_000_000_000
}

// ---------- the original on Hugging Face ----------

/// One of the original's files.
#[derive(Debug, Clone, PartialEq)]
struct OriginalFile {
    path: String,
    size: u64,
    /// Hugging Face's checksum, for the big (LFS) files; the small settings
    /// and tokenizer files have none and come from a fixed revision.
    sha256: Option<String>,
}

/// The model a GGUF upload was made from ("base_model:quantized:<repo>" in
/// its tags, as Hugging Face links them), or the upload itself when it
/// has weights to change.
fn original_in(info: &Value) -> Option<String> {
    let tags = info["tags"].as_array().into_iter().flatten().filter_map(Value::as_str);
    if let Some(r) = tags.clone().find_map(|t| t.strip_prefix("base_model:quantized:")) {
        return Some(r.to_string());
    }
    // A finetune, adapter or merge of its base: the base named in its card
    // isn't what it was made from (the copy would lose what was added).
    if tags.clone().any(|t| ["base_model:finetune:", "base_model:adapter:", "base_model:merge:"].iter().any(|p| t.starts_with(p))) {
        return None;
    }
    match &info["cardData"]["base_model"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) if a.len() == 1 => a[0].as_str().map(str::to_string),
        _ => None,
    }
}

/// Settings and tokenizer files the converter reads, beside the weights.
fn is_setting(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".json", ".txt", ".model", ".jinja", ".tiktoken"].iter().any(|e| lower.ends_with(e)) && !lower.starts_with("readme") && !lower.starts_with("license")
}

/// The files a run downloads: the weights (the ones the index names, when
/// there is one: some uploads also carry the same weights in another
/// layout) and the settings and tokenizer files at the top of the upload.
fn wanted(siblings: &[OriginalFile], index: Option<&Value>) -> Vec<OriginalFile> {
    let top = siblings.iter().filter(|f| !f.path.contains('/'));
    let shards: Option<Vec<String>> = index.and_then(|i| i["weight_map"].as_object()).map(|m| {
        let mut v: Vec<String> = m.values().filter_map(Value::as_str).map(str::to_string).collect();
        v.sort();
        v.dedup();
        v
    });
    let weight = |f: &OriginalFile| match &shards {
        Some(s) => s.contains(&f.path),
        None => f.path == "model.safetensors",
    };
    top.filter(|f| weight(f) || f.path.ends_with(".safetensors.index.json") || (is_setting(&f.path) && !f.path.ends_with(".safetensors"))).cloned().collect()
}

fn weights_size(files: &[OriginalFile]) -> u64 {
    files.iter().filter(|f| f.path.ends_with(".safetensors")).map(|f| f.size).sum()
}

/// What Hugging Face says of a repository: its files, at its latest revision.
async fn repo_info(client: &reqwest::Client, repo: &str) -> Result<Value, String> {
    let resp = client.get(format!("{API}/{repo}?blobs=true")).send().await.map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND || resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(format!("Hugging Face has no model called {repo} (or it's private)."));
    }
    resp.error_for_status().map_err(|e| format!("Hugging Face answered: {e}"))?.json().await.map_err(|e| e.to_string())
}

fn siblings(info: &Value) -> Vec<OriginalFile> {
    info["siblings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(OriginalFile {
                path: s["rfilename"].as_str()?.to_string(),
                size: s["lfs"]["size"].as_u64().or_else(|| s["size"].as_u64())?,
                sha256: s["lfs"]["sha256"].as_str().map(str::to_string),
            })
        })
        .collect()
}

fn file_url(repo: &str, revision: &str, path: &str) -> String {
    format!("{SITE}/{repo}/resolve/{revision}/{path}")
}

/// A small file of the original's (settings, the weights' index), read.
async fn small_json(client: &reqwest::Client, repo: &str, revision: &str, path: &str) -> Result<Value, String> {
    crate::offline::check(crate::offline::Keep::Downloads)?;
    let resp = client.get(file_url(repo, revision, path)).send().await.map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?;
    let bytes = resp.error_for_status().map_err(|e| format!("Hugging Face answered: {e}"))?.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 20 << 20 {
        return Err(format!("{path} is too big for a settings file."));
    }
    serde_json::from_slice(&bytes).map_err(|_| format!("{path} isn't readable."))
}

/// Why a model's original can't be changed here, from its settings.
fn config_problem(config: &Value) -> Option<String> {
    if !config["auto_map"].is_null() {
        return Some("That model runs code of its own to load, which SulcusAI doesn't run. Pick another model.".into());
    }
    let method = config["quantization_config"]["quant_method"].as_str().or_else(|| config["text_config"]["quantization_config"]["quant_method"].as_str());
    match method {
        None | Some("mxfp4") => None,
        Some(m) => Some(format!("That upload is already compressed ({m}), and Heretic needs the full weights. Paste the link to the model's original upload.")),
    }
}

/// The original, looked at: what a run would download and whether it can.
struct Original {
    repo: String,
    revision: String,
    files: Vec<OriginalFile>,
    license: Option<String>,
}

async fn fetch_original(client: &reqwest::Client, repo: &str) -> Result<Original, String> {
    let info = repo_info(client, repo).await?;
    if !(info["gated"].is_null() || info["gated"] == false) {
        return Err(format!("{repo} needs a Hugging Face account to download, which SulcusAI doesn't sign in to. Many models have an open copy (unsloth's often are): paste the link to one."));
    }
    let revision = info["sha"].as_str().ok_or("Hugging Face didn't say which revision that is.")?.to_string();
    let all = siblings(&info);
    let index = match all.iter().find(|f| f.path == "model.safetensors.index.json") {
        Some(f) => Some(small_json(client, repo, &revision, &f.path).await?),
        None => None,
    };
    let files = wanted(&all, index.as_ref());
    if weights_size(&files) == 0 {
        return Err(format!("{repo} has no full weights (.safetensors) to change, only versions made for running. Paste the link to the model's original upload."));
    }
    if !files.iter().any(|f| f.path == "config.json") {
        return Err(format!("{repo} has no config.json, so it can't be loaded."));
    }
    let config = small_json(client, repo, &revision, "config.json").await?;
    if let Some(why) = config_problem(&config) {
        return Err(why);
    }
    let license = info["cardData"]["license"].as_str().map(str::to_string);
    Ok(Original { repo: repo.to_string(), revision, files, license })
}

/// The original a model was made from, for the dialog to suggest: the
/// GGUF upload's base model on Hugging Face.
async fn suggest(client: &reqwest::Client, spec: &crate::catalog::ModelSpec) -> Option<String> {
    if !spec.source.starts_with(SITE) {
        return None;
    }
    let (repo, _) = crate::hub::parse_link(&spec.source).ok()?;
    let info = repo_info(client, &repo).await.ok()?;
    if siblings(&info).iter().any(|f| f.path.ends_with(".safetensors")) {
        return Some(repo);
    }
    original_in(&info)
}

// ---------- will it fit ----------

/// Card memory a run takes besides the weights: the tries' answers (100 at
/// a time), the change being tried and PyTorch's own. Qwen2.5 0.5B held
/// 0.93 GB of weights and peaked under 3 GB at 128 answers at once.
const WORK: u64 = 2_500 << 20;
/// Card memory the desktop takes at the least.
const DESKTOP: u64 = 512 << 20;

/// How the model is loaded on the card: as it is, or squeezed to 4 bits
/// a weight (about a quarter of the memory, a little slower and less exact)
/// when it doesn't fit as it is. In 4 bits, saving the copy loads the whole
/// model in the computer's memory to put the change in (Heretic: about 3
/// times its billions of weights in GB, so 1.5 times its 16-bit size).
fn plan(weights: u64, card: u64, ram: u64) -> Result<bool, String> {
    if weights + WORK + DESKTOP <= card {
        return Ok(false);
    }
    let four_bit = weights * 3 / 10 + WORK + DESKTOP;
    if four_bit > card {
        return Err("This model is too big to change on this graphics card, even squeezed to 4 bits. Pick a smaller model.".into());
    }
    if weights * 3 / 2 + (4 << 30) > ram {
        return Err("This model only fits the graphics card squeezed to 4 bits, and putting the change into it then needs more of the computer's memory than this PC has. Pick a smaller model.".into());
    }
    Ok(true)
}

/// Disk space a run needs at its fullest: the original, and the saved copy
/// beside it (then the copy and its GGUF), and the finished file.
fn disk_need(weights: u64, finished: u64) -> u64 {
    weights * 2 + finished + (1 << 30)
}

/// About how long a run takes, in seconds: loading and finding the
/// direction, then the tries (4.5 s each for a 1 GB model on an RTX 5070,
/// growing with the model), then saving, converting and squeezing.
fn estimate(weights: u64, trials: u32, four_bit: bool) -> u64 {
    let gb = weights as f64 / 1e9;
    let per_try = (2.5 + 2.0 * gb) * if four_bit { 1.6 } else { 1.0 };
    (120.0 + per_try * trials as f64 + 30.0 * gb) as u64
}

/// The quantization the copy is squeezed to: the installed model's, when
/// llama-quantize makes it without a sample of text to steer it (the
/// "IQ" kinds need one); else Q4_K_M.
fn target_quant(installed: &str) -> String {
    const PLAIN: &[&str] = &["Q8_0", "Q6_K", "Q5_K_M", "Q5_K_S", "Q5_0", "Q4_K_M", "Q4_K_S", "Q4_0", "Q3_K_L", "Q3_K_M", "Q3_K_S", "Q2_K", "F16", "BF16"];
    let upper = installed.to_ascii_uppercase();
    // unsloth's dynamic quantizations ("UD-Q5_K_XL") are built on a plain
    // kind: the nearest one, not Q4_K_M (a Q6 model came out a Q4 copy).
    let plain = upper.strip_prefix("UD-").unwrap_or(&upper);
    let q = match plain {
        "Q8_K_XL" => "Q8_0",
        "Q6_K_XL" => "Q6_K",
        "Q5_K_XL" => "Q5_K_M",
        "Q4_K_XL" => "Q4_K_M",
        "Q3_K_XL" => "Q3_K_L",
        "Q2_K_XL" => "Q2_K",
        other => other,
    }
    .to_string();
    if PLAIN.contains(&q.as_str()) {
        q
    } else {
        "Q4_K_M".into()
    }
}

// ---------- the job ----------

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    /// The model being copied.
    pub model_id: String,
    pub name: String,
    /// setting_up, downloading, loading, trying, saving, converting, done,
    /// failed, cancelled.
    pub status: String,
    pub fraction: f64,
    pub trial: u32,
    pub trials: u32,
    /// The original's refusals of the test requests, and how many there are.
    pub base_refusals: Option<u32>,
    pub of: u32,
    /// The best try so far within the drift allowed (at the end, the one
    /// kept): its refusals and drift.
    pub refusals: Option<u32>,
    pub kl: Option<f64>,
    pub four_bit: bool,
    pub seconds_left: Option<u64>,
    /// While the original downloads: bytes so far, of how many, and the
    /// speed in bytes a second.
    pub received: Option<u64>,
    pub total: Option<u64>,
    pub bytes_per_sec: Option<f64>,
    pub error: Option<String>,
    /// The copy, once it's in.
    pub copy_id: Option<String>,
}

static JOB: Mutex<Option<(Job, Arc<AtomicBool>)>> = Mutex::new(None);

fn done(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled")
}

/// A copy is being made right now (it has the graphics card).
pub fn running() -> bool {
    JOB.lock().unwrap().as_ref().is_some_and(|(j, _)| !done(&j.status))
}

pub const BUSY: &str = "An uncensored copy of a model is being made and has the graphics card. Try again when it's done (or stop it on the Models page).";

/// Changes the job and tells the window (when there is one: tests have none).
fn update(state: &AppState, f: impl FnOnce(&mut Job)) {
    let snapshot = {
        let mut g = JOB.lock().unwrap();
        let Some((job, _)) = g.as_mut() else { return };
        f(job);
        job.clone()
    };
    if let Some(app) = state.app.get() {
        app.emit("uncensor:progress", &snapshot).ok();
    }
}

/// What a line of the runner's output says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Said {
    Stage(&'static str),
    /// The original's refusals, of how many test requests.
    Base(u32, u32),
    /// A try starting, of how many.
    Trial(u32, u32),
    /// A try's refusals and drift.
    Tried(u32, f64),
    /// The try kept: its number, refusals and drift.
    Chose(u32, u32, f64),
    Saved,
}

pub fn said(line: &str) -> Option<Said> {
    let rest = line.trim().strip_prefix("SULCUS ")?;
    let w: Vec<&str> = rest.split_whitespace().collect();
    let n = |i: usize| w.get(i).and_then(|s| s.parse::<u32>().ok());
    let f = |i: usize| w.get(i).and_then(|s| s.parse::<f64>().ok());
    match *w.first()? {
        "stage" => match *w.get(1)? {
            "prompts" | "batch" | "directions" => Some(Said::Stage("loading")),
            "saving" => Some(Said::Stage("saving")),
            _ => None,
        },
        "base" => Some(Said::Base(n(1)?, n(2)?)),
        "trial" => Some(Said::Trial(n(1)?, n(2)?)),
        "tried" => Some(Said::Tried(n(1)?, f(2)?)),
        "chose" => Some(Said::Chose(n(1)?, n(2)?, f(3)?)),
        "saved" => Some(Said::Saved),
        _ => None,
    }
}

/// The runner's arguments (to uv): the original in, the copy out.
pub fn args(source: &Path, model: &Path, out: &Path, work: &Path, prompts: &Path, trials: u32, four_bit: bool) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["run".into(), "--frozen".into(), "--no-sync".into(), "--project".into(), source.into(), "python".into(), source.join("uncensor.py").into()];
    a.extend(["--model".into(), model.into(), "--out".into(), out.into(), "--work".into(), work.into(), "--prompts".into(), prompts.into()]);
    a.extend(["--trials".into(), trials.clamp(10, 400).to_string().into(), "--max-kl".into(), MAX_KL.to_string().into()]);
    if four_bit {
        a.push("--quantize".into());
    }
    a
}

/// The converter's arguments (to uv): the saved copy in, a 16-bit GGUF out,
/// named in its header (else it's named after the folder).
pub fn convert_args(source: &Path, saved: &Path, out: &Path, name: &str) -> Vec<OsString> {
    let script = source.join("llama-convert").join("convert_hf_to_gguf.py");
    let mut a: Vec<OsString> = vec!["run".into(), "--frozen".into(), "--no-sync".into(), "--project".into(), source.into(), "python".into(), script.into(), saved.into()];
    a.extend(["--outfile".into(), out.into(), "--outtype".into(), "bf16".into(), "--model-name".into(), name.into()]);
    a
}

/// The environment they run in: the music trainer's (uv's Python, packages
/// and cache, nothing fetched while they run), progress bars quiet.
fn env(paths: &Paths) -> Vec<(&'static str, OsString)> {
    let mut e = train::trainer_env(paths);
    e.push(("TQDM_DISABLE", "1".into()));
    e.push(("HF_DATASETS_OFFLINE", "1".into()));
    e
}

/// The copy's name in the model menu.
pub fn copy_name(name: &str) -> String {
    format!("{name} (uncensored)")
}

// ---------- commands ----------

/// What the dialog shows before a run.
#[derive(Debug, Serialize)]
pub struct Look {
    /// The original on Hugging Face ("owner/name"), as found or pasted.
    original: Option<String>,
    /// Why it can't be made from that original (a whole sentence).
    problem: Option<String>,
    /// What the run downloads (the original's weights), and the disk space
    /// it needs at its fullest, and what's free there.
    download: u64,
    disk: u64,
    free: Option<u64>,
    /// Loaded in 4 bits to fit the graphics card.
    four_bit: bool,
    /// Setting Heretic up the first time downloads about this much (0 once
    /// it's set up).
    setup: u64,
    /// About how long, in seconds, quick and thorough.
    quick: u64,
    thorough: u64,
    /// The finished copy's size, about.
    size: u64,
    /// What stopped runs left on the disk.
    leftover: u64,
    job: Option<Job>,
}

fn require(state: &AppState) -> Result<&HereticCatalog, String> {
    if !crate::advanced::stored(&state.db.lock().unwrap()).enabled {
        return Err("Uncensored copies are in Advanced mode. Turn it on in Settings → Advanced first.".into());
    }
    let h = state.catalog.heretic.as_ref().ok_or("Uncensored copies aren't available on this system yet.")?;
    if !cfg!(windows) {
        return Err("Uncensored copies aren't available on this system yet.".into());
    }
    if !crate::natural::nvidia_here(state) {
        return Err("Making an uncensored copy needs an NVIDIA graphics card.".into());
    }
    Ok(h)
}

/// What keeps this PC's NVIDIA card from running Heretic: PyTorch for CUDA
/// 12.8 needs driver 570 or newer and a GeForce RTX 20 card or newer. Found
/// before anything downloads (it failed only once the model was loading).
async fn card_problem() -> Option<String> {
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=driver_version,compute_cap", "--format=csv,noheader"]).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(std::time::Duration::from_secs(5), cmd.output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let best = text
        .lines()
        .filter_map(|l| {
            let (d, c) = l.split_once(',')?;
            Some((d.trim().split('.').next()?.parse::<u32>().ok()?, c.trim().parse::<f64>().ok()?))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if best.0 < 570 {
        return Some(format!("Making an uncensored copy needs NVIDIA's driver 570 or newer, and this PC has {}. Update the driver, then try again.", best.0));
    }
    if best.1 < 7.5 {
        return Some("Making an uncensored copy needs a GeForce RTX 20 series card or newer.".into());
    }
    None
}

/// The card memory a run can have now: what's free, with what the desktop
/// keeps and what SulcusAI's own chat model gives back (it stops for the run).
async fn card_for_run(state: &AppState, card: u64) -> u64 {
    let given_back = state.engine.lock().await.loaded().and_then(|(_, id, quant, ctx, layers)| state.model_spec(&id).map(|s| crate::memcheck::need(&s, &quant, ctx, layers, 0, false).vram)).unwrap_or(0);
    crate::finetune::card_free(state).await.map_or(card, |f| (f + DESKTOP + given_back).min(card))
}

/// What a run found, kept beside the saved copy, so a run stopped after it
/// was saved carries on from there with its figures.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq)]
struct Found {
    base: Option<u32>,
    of: u32,
    refusals: Option<u32>,
    kl: Option<f64>,
    four_bit: bool,
}

/// A try that took out too little to be worth keeping: it still refuses
/// most of what the original refused.
fn too_little(base: Option<u32>, refusals: Option<u32>) -> bool {
    matches!((base, refusals), (Some(b), Some(r)) if b >= 10 && r * 10 >= b * 8)
}

fn leftover(paths: &Paths) -> u64 {
    crate::storage::folder_size(&work_root(paths))
}

/// Looks at a model's original: the one pasted, or else the one its upload
/// names.
#[tauri::command]
pub async fn uncensor_look(state: AppStateRef<'_>, model_id: String, original: Option<String>) -> Result<Look, String> {
    let h = require(&state)?.clone();
    let installed = db::installed_model(&state.db.lock().unwrap(), &model_id).ok_or("Pick a model that's installed.")?;
    let spec = state.model_spec(&model_id).ok_or("Unknown model")?;
    let client = crate::hub::client(&state)?;
    let original = match original.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => Some(crate::hub::parse_link(text)?.0),
        None => suggest(&client, &spec).await,
    };
    let job = JOB.lock().unwrap().as_ref().map(|(j, _)| j.clone());
    let setup = if ready(&state.paths, &h) { 0 } else { setup_bytes(&h) };
    let mut look = Look { original: original.clone(), problem: None, download: 0, disk: 0, free: None, four_bit: false, setup, quick: 0, thorough: 0, size: 0, leftover: leftover(&state.paths), job };
    let Some(repo) = original else {
        look.problem = Some("SulcusAI couldn't tell which model on Hugging Face this one was made from. Paste the link to its original upload (the one with .safetensors files).".into());
        return Ok(look);
    };
    let o = match fetch_original(&client, &repo).await {
        Ok(o) => o,
        Err(e) => {
            look.problem = Some(e);
            return Ok(look);
        }
    };
    let weights = weights_size(&o.files);
    let (card, ram) = {
        let hw = state.hardware.read().unwrap();
        (hw.best_gpu().map_or(0, |g| g.vram_bytes), hw.ram_total)
    };
    look.download = o.files.iter().map(|f| f.size).sum();
    look.size = installed.size;
    look.disk = disk_need(weights, installed.size);
    look.free = crate::storage::free_on(&state.paths.models);
    if let Some(p) = card_problem().await {
        look.problem = Some(p);
        return Ok(look);
    }
    // As the card is now (a game or another app may be using some of it).
    match plan(weights, card_for_run(&state, card).await, ram) {
        Ok(four_bit) => look.four_bit = four_bit,
        Err(e) => look.problem = Some(e),
    }
    if look.problem.is_none() && look.free.is_some_and(|f| f < look.disk) {
        look.problem = Some(format!("It needs about {} free on the drive the models are on, and {} is free.", crate::size_label(look.disk), crate::size_label(look.free.unwrap_or(0))));
    }
    look.quick = estimate(weights, QUICK, look.four_bit);
    look.thorough = estimate(weights, THOROUGH, look.four_bit);
    Ok(look)
}

/// Starts making the copy. `pin`: the Advanced settings' PIN, when one is set.
#[tauri::command]
pub async fn start_uncensor(state: AppStateRef<'_>, model_id: String, original: String, thorough: bool, pin: Option<String>) -> Result<(), String> {
    state.cipher()?;
    require(&state)?;
    crate::advanced::check_lock(&state.db.lock().unwrap(), pin.as_deref())?;
    if crate::finetune::training() {
        return Err("A model is being taught right now and has the graphics card. Make the copy once that's done.".into());
    }
    if crate::media::any_job() {
        return Err("A picture, video or song is being made on the graphics card. Make the copy once that's done.".into());
    }
    db::installed_model(&state.db.lock().unwrap(), &model_id).ok_or("Pick a model that's installed.")?;
    let repo = crate::hub::parse_link(&original)?.0;
    let trials = if thorough { THOROUGH } else { QUICK };
    let cancel = begin(&state, &model_id, trials)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run(&state, &model_id, &repo, trials, &cancel).await;
    });
    Ok(())
}

/// Takes the job for a copy of `model_id`, unless one is being made. Its
/// stop flag.
pub(crate) fn begin(state: &AppState, model_id: &str, trials: u32) -> Result<Arc<AtomicBool>, String> {
    let spec = state.model_spec(model_id).ok_or("Unknown model")?;
    let job = Job {
        model_id: model_id.to_string(),
        name: spec.name,
        status: "setting_up".into(),
        fraction: 0.0,
        trial: 0,
        trials,
        base_refusals: None,
        of: 0,
        refusals: None,
        kl: None,
        four_bit: false,
        seconds_left: None,
        received: None,
        total: None,
        bytes_per_sec: None,
        error: None,
        copy_id: None,
    };
    let mut g = JOB.lock().unwrap();
    if g.as_ref().is_some_and(|(j, _)| !done(&j.status)) {
        return Err("A copy is already being made. Wait for it or stop it first.".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *g = Some((job, cancel.clone()));
    Ok(cancel)
}

/// Makes the copy (the job `begin` took), from `repo` on Hugging Face, and
/// ends the job. The copy's id.
pub(crate) async fn run(state: &Arc<AppState>, model_id: &str, repo: &str, trials: u32, cancel: &AtomicBool) -> Result<String, String> {
    // The window shows it at once, before the first step reports.
    update(state, |_| {});
    // A big model's run takes hours; sleeping would cut it off.
    let _awake = crate::awake::KeepAwake::new();
    let made = async {
        let h = state.catalog.heretic.clone().ok_or("Uncensored copies aren't available on this system yet.")?;
        let installed = db::installed_model(&state.db.lock().unwrap(), model_id).ok_or("Pick a model that's installed.")?;
        let spec = state.model_spec(model_id).ok_or("Unknown model")?;
        state.log("model", &format!("Started making an uncensored copy of {} from {repo} ({trials} tries)", spec.name));
        make(state, &h, &installed, &spec, repo, trials, cancel).await
    }
    .await;
    let name = JOB.lock().unwrap().as_ref().map(|(j, _)| j.name.clone()).unwrap_or_default();
    match &made {
        Ok(id) => {
            state.log("model", &format!("Made {}; pick it in a chat's model menu", copy_name(&name)));
            update(state, |j| {
                j.status = "done".into();
                j.fraction = 1.0;
                j.seconds_left = None;
                j.copy_id = Some(id.clone());
            });
        }
        Err(e) if e == crate::download::CANCELLED => {
            state.log("model", &format!("Stopped making an uncensored copy of {name} (what it downloaded and the tries so far are kept, so starting again carries on)"));
            update(state, |j| j.status = "cancelled".into());
        }
        Err(e) => {
            state.log("model", &format!("Making an uncensored copy of {name} failed: {e}"));
            update(state, |j| {
                j.status = "failed".into();
                j.error = Some(e.clone());
            });
        }
    }
    if let Some(app) = state.app.get() {
        app.emit("features:changed", json!({})).ok();
    }
    made
}

#[tauri::command(async)]
pub fn cancel_uncensor() {
    if let Some((_, cancel)) = JOB.lock().unwrap().as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command(async)]
pub fn uncensor_job() -> Option<Job> {
    JOB.lock().unwrap().as_ref().map(|(j, _)| j.clone())
}

/// Deletes what stopped runs left (the originals they downloaded and their
/// tries). Returns the bytes freed.
#[tauri::command(async)]
pub fn uncensor_clear(state: AppStateRef) -> Result<u64, String> {
    if running() {
        return Err("A copy is being made. Stop it first.".into());
    }
    let root = work_root(&state.paths);
    let freed = crate::storage::folder_size(&root);
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(|e| format!("Couldn't delete it: {e}"))?;
    }
    Ok(freed)
}

/// At startup: what runs stopped more than three days ago left behind.
pub fn clean_leftovers(paths: &Paths) {
    let stale = std::time::Duration::from_secs(3 * 86_400);
    let Ok(entries) = std::fs::read_dir(work_root(paths)) else { return };
    for e in entries.flatten() {
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > stale);
        if old {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
}

// ---------- making it ----------

/// Heretic, set up the first time: uv (the music trainer's), the runner
/// with its locked Python packages and converter, and the prompt sets.
/// Returns uv.
async fn ensure_heretic(state: &AppState, h: &HereticCatalog, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let paths = &state.paths;
    let uv = engine::ensure_unpacked(paths, "trainer-uv-bin", &h.uv, "uv.exe", cancel, |r, all| update(state, |j| j.fraction = 0.005 * r as f64 / all.max(1) as f64)).await?;
    let source = source_dir(paths, h);
    let dir_name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
    engine::ensure_unpacked(paths, &dir_name, &h.source, "uncensor.py", cancel, |_, _| {}).await?;
    if !source.join(".venv").join("Scripts").join("python.exe").is_file() {
        // Its Python and packages (PyTorch for NVIDIA cards and the rest), as locked.
        state.log("network", "Downloading Heretic's Python packages from pypi.org and download.pytorch.org");
        update(state, |j| j.fraction = 0.01);
        let ran = proc::run_env(
            Run { exe: &uv, args: ["sync", "--frozen", "--python", "3.12"].map(OsString::from).to_vec(), cwd: &source, log: &paths.logs.join("uncensor-setup.log"), low_priority: state.limits().low_priority, job: state.job() },
            &env(paths),
            cancel,
            |_| {},
        )
        .await;
        if let Err(e) = ran {
            // A half-made environment would look ready next time.
            let _ = std::fs::remove_dir_all(source.join(".venv"));
            return Err(e);
        }
    }
    let dir = prompts_dir(paths, h);
    let missing: Vec<&TrainerFile> = h.prompts.iter().filter(|f| !dir.join(&f.file).is_file()).collect();
    if !missing.is_empty() {
        state.log("network", "Downloading Heretic's prompt sets from huggingface.co");
        let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false)?;
        for f in missing {
            crate::download::fetch_verified(&client, &f.url, &dir.join(&f.file), f.size, &f.sha256, cancel, |_, _| {}).await?;
        }
    }
    update(state, |j| j.fraction = 0.03);
    Ok(uv)
}

/// The original's files, downloaded into `dir` (a stopped run's are kept
/// and checked, not fetched again).
async fn download(state: &AppState, o: &Original, dir: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let client = crate::hub::client(state)?;
    let total: u64 = o.files.iter().map(|f| f.size).sum::<u64>().max(1);
    state.log("network", &format!("Downloading {} ({}) from huggingface.co to make an uncensored copy", o.repo, crate::size_label(total)));
    let mut got = 0u64;
    let mut speed = Speed::default();
    for f in &o.files {
        let dest = dir.join(&f.path);
        let url = file_url(&o.repo, &o.revision, &f.path);
        let base = got;
        // A file already downloaded (or resumed) jumps ahead at once; that isn't speed.
        speed.since = None;
        let progress = |r: u64, _| {
            let rate = speed.add(base + r, Instant::now());
            update(state, |j| {
                j.fraction = 0.03 + 0.12 * (base + r) as f64 / total as f64;
                j.received = Some(base + r);
                j.total = Some(total);
                if rate.is_some() {
                    j.bytes_per_sec = rate;
                }
            })
        };
        match &f.sha256 {
            Some(sha) => crate::download::fetch_verified(&client, &url, &dest, f.size, sha, cancel, progress).await?,
            None => fetch_small(&client, &url, &dest, f.size).await?,
        }
        got += f.size;
    }
    update(state, |j| {
        j.received = None;
        j.total = None;
        j.bytes_per_sec = None;
    });
    Ok(())
}

/// Download speed over the last second or so.
#[derive(Default)]
struct Speed {
    since: Option<(Instant, u64)>,
}

impl Speed {
    /// The speed, once a second has gone by since the last one.
    fn add(&mut self, bytes: u64, now: Instant) -> Option<f64> {
        let Some((t, b)) = self.since else {
            self.since = Some((now, bytes));
            return None;
        };
        let secs = now.duration_since(t).as_secs_f64();
        if secs < 1.0 {
            return None;
        }
        self.since = Some((now, bytes));
        Some(bytes.saturating_sub(b) as f64 / secs)
    }
}

/// A settings or tokenizer file (Hugging Face has no checksum for these;
/// the revision in the link fixes what they hold).
async fn fetch_small(client: &reqwest::Client, url: &str, dest: &Path, size: u64) -> Result<(), String> {
    if std::fs::metadata(dest).is_ok_and(|m| m.len() == size) {
        return Ok(());
    }
    crate::offline::check(crate::offline::Keep::Downloads)?;
    if size > 64 << 20 {
        return Err("One of the model's settings files is too big.".into());
    }
    let bytes = client.get(url).send().await.map_err(|e| format!("Download failed: {e}"))?.error_for_status().map_err(|e| format!("Download failed: {e}"))?.bytes().await.map_err(|e| format!("Download failed: {e}"))?;
    if bytes.len() as u64 != size {
        return Err("A settings file of the model came out a different size than Hugging Face lists. Try again.".into());
    }
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(dest, &bytes).map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn make(state: &Arc<AppState>, h: &HereticCatalog, installed: &InstalledModel, spec: &crate::catalog::ModelSpec, repo: &str, trials: u32, cancel: &AtomicBool) -> Result<String, String> {
    let paths = &state.paths;
    if let Some(p) = card_problem().await {
        return Err(p);
    }
    let o = fetch_original(&crate::hub::client(state)?, repo).await?;
    let weights = weights_size(&o.files);
    let (card, ram) = {
        let hw = state.hardware.read().unwrap();
        (hw.best_gpu().map_or(0, |g| g.vram_bytes), hw.ram_total)
    };
    // Room on the card, before anything downloads.
    plan(weights, card_for_run(state, card).await, ram)?;
    let uv = ensure_heretic(state, h, cancel).await?;
    let source = source_dir(paths, h);
    let work = work_dir(paths, &o.repo, &o.revision);
    // Only one run's leftovers at a time: another model's go.
    if let Ok(entries) = std::fs::read_dir(work_root(paths)) {
        for e in entries.flatten().filter(|e| e.path() != work) {
            std::fs::remove_dir_all(e.path()).ok();
        }
    }
    let need = disk_need(weights, installed.size).saturating_sub(crate::storage::folder_size(&work));
    if crate::storage::free_on(&paths.models).is_some_and(|free| free < need) {
        return Err(format!("It needs about {} more free space on the drive the models are on.", crate::size_label(need)));
    }
    let original_dir = work.join("original");
    let saved = work.join("saved");
    let full = work.join("copy-bf16.gguf");
    let (saved_ok, full_ok) = (work.join("saved.ok"), work.join("bf16.ok"));
    // A run stopped after the copy was saved (or converted) carries on from
    // there: the original isn't downloaded again, nor the tries made again.
    let have_full = full_ok.is_file() && full.is_file();
    let carried: Option<Found> = std::fs::read_to_string(&saved_ok).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(|_| have_full || saved.join("config.json").is_file());
    if let Some(f) = &carried {
        state.log("model", "Carrying on from the copy a stopped run saved (no download or tries needed)");
        update(state, |j| {
            j.base_refusals = f.base;
            j.of = f.of;
            j.refusals = f.refusals;
            j.kl = f.kl;
            j.four_bit = f.four_bit;
        });
    } else {
    update(state, |j| j.status = "downloading".into());
    download(state, &o, &original_dir, cancel).await?;

    // The chat model gives the graphics card back for the run.
    state.engine.lock().await.stop().await;
    let free = crate::finetune::card_free(state).await.map_or(card, |f| f + DESKTOP);
    let four_bit = plan(weights, free.min(card), ram)?;
    update(state, |j| {
        j.status = "loading".into();
        j.four_bit = four_bit;
        j.fraction = 0.15;
    });
    std::fs::remove_dir_all(&saved).ok();
    std::fs::remove_file(&full_ok).ok();
    let tries = Mutex::new(None::<(u32, Instant)>);
    let tail = (30.0 * weights as f64 / 1e9) as u64;
    let log = paths.logs.join("uncensor.log");
    // The runner's own reason when it stops; running out of card memory
    // counts only once Heretic has picked its batch size (it runs out on
    // purpose while looking for it).
    let mut runner_error: Option<String> = None;
    let (mut sized, mut oom) = (false, false);
    let ran = proc::run_env(
        Run { exe: &uv, args: args(&source, &original_dir, &saved, &work.join("heretic"), &prompts_dir(paths, h), trials, four_bit), cwd: &work, log: &log, low_priority: state.limits().low_priority, job: state.job() },
        &env(paths),
        cancel,
        |line| {
            if let Some(why) = line.trim().strip_prefix("SULCUS error ") {
                runner_error = Some(why.to_string());
            }
            if line.contains("Chosen batch size") {
                sized = true;
            }
            if sized && (line.contains("CUDA out of memory") || line.contains("OutOfMemoryError")) {
                oom = true;
            }
            let Some(s) = said(line) else { return };
            match s {
                Said::Stage(stage) => update(state, |j| {
                    j.status = stage.into();
                    if stage == "saving" {
                        j.fraction = 0.9;
                        j.seconds_left = Some(tail);
                    }
                }),
                Said::Base(b, of) => update(state, |j| {
                    j.base_refusals = Some(b);
                    j.of = of;
                }),
                Said::Trial(n, of) => {
                    // Timed from the end of this run's first try (it includes
                    // warming up); a run that carries on starts past try 1.
                    let mut t = tries.lock().unwrap();
                    let (first, started) = *t.get_or_insert((n, Instant::now()));
                    if n == first + 1 {
                        *t = Some((first, Instant::now()));
                    }
                    let made = n.saturating_sub(first + 1);
                    let left = (made > 0).then(|| (started.elapsed().as_secs_f64() / made as f64 * (of + 1).saturating_sub(n) as f64) as u64 + tail);
                    update(state, |j| {
                        j.status = "trying".into();
                        j.trial = n;
                        j.trials = of;
                        j.fraction = 0.2 + 0.7 * (n - 1) as f64 / of.max(1) as f64;
                        if left.is_some() {
                            j.seconds_left = left;
                        }
                    });
                }
                Said::Tried(r, kl) => update(state, |j| {
                    if kl <= MAX_KL && j.refusals.is_none_or(|best| r < best) {
                        j.refusals = Some(r);
                        j.kl = Some(kl);
                    }
                }),
                Said::Chose(_, r, kl) => update(state, |j| {
                    j.refusals = Some(r);
                    j.kl = Some(kl);
                }),
                Said::Saved => update(state, |j| j.fraction = 0.93),
            }
        },
    )
    .await;
    if let Err(e) = ran {
        if e == crate::download::CANCELLED {
            return Err(e);
        }
        if let Some(why) = runner_error {
            return Err(why);
        }
        if oom {
            return Err("The graphics card ran out of memory. Close other apps that use it, such as games, browsers and video players, and try again.".into());
        }
        return Err(format!("Heretic stopped with an error ({e}). Its log (uncensor.log) is in the app's logs folder."));
    }
    if !saved.join("config.json").is_file() {
        return Err("Heretic finished without saving the copy. Its log (uncensor.log) is in the app's logs folder.".into());
    }
    let found = JOB.lock().unwrap().as_ref().map(|(j, _)| Found { base: j.base_refusals, of: j.of, refusals: j.refusals, kl: j.kl, four_bit }).unwrap_or_default();
    if too_little(found.base, found.refusals) {
        std::fs::remove_dir_all(&work).ok();
        return Err(format!(
            "Heretic couldn't take enough of this model's refusals out: the best try still refused {} of {} test requests (the original refused {}). A Thorough run tries more ways.",
            found.refusals.unwrap_or(0),
            found.of,
            found.base.unwrap_or(0)
        ));
    }
    let _ = std::fs::write(&saved_ok, serde_json::to_string(&found).unwrap_or_default());
    // The original's download isn't needed any more; its space goes to the conversion.
    std::fs::remove_dir_all(&original_dir).ok();
    }

    update(state, |j| {
        j.status = "converting".into();
        j.fraction = 0.94;
    });
    let log = paths.logs.join("uncensor.log");
    if !(full_ok.is_file() && full.is_file()) {
        proc::run_env(Run { exe: &uv, args: convert_args(&source, &saved, &full, &copy_name(&spec.name)), cwd: &work, log: &log, low_priority: true, job: state.job() }, &env(paths), cancel, |_| {}).await?;
        if !full.is_file() {
            return Err("Turning the copy into a GGUF failed. The log (uncensor.log) is in the app's logs folder.".into());
        }
        let _ = std::fs::write(&full_ok, "");
    }
    std::fs::remove_dir_all(&saved).ok();
    update(state, |j| j.fraction = 0.97);

    let quant = target_quant(&installed.quant);
    let id = format!("made-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let dir = paths.models.join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let file = dir.join(format!("{name}-uncensored-{quant}.gguf"));
    let quantize = engine::installed_server(paths, &state.catalog.engine, engine::backend_for(&state.budget())?)
        .and_then(|server| engine::find_exe(server.parent()?, if cfg!(windows) { "llama-quantize.exe" } else { "llama-quantize" }))
        .ok_or("The engine's model converter (llama-quantize) is missing. Send a chat message once so the engine downloads, then try again.")?;
    let threads = state.limits().threads.max(1).to_string();
    let quantized = proc::run(Run { exe: &quantize, args: vec![full.clone().into(), file.clone().into(), quant.clone().into(), threads.into()], cwd: &work, log: &log, low_priority: true, job: state.job() }, cancel, |_| {}).await;
    if let Err(e) = quantized {
        std::fs::remove_dir_all(&dir).ok();
        return Err(e);
    }

    // Its picture reader, when the original has one: the change is in the
    // language part only, so the original's reads pictures for it as well.
    let vision = match (crate::vision_file(paths, spec), &spec.vision) {
        (Some(from), Some(v)) => {
            let to_name = from.file_name().unwrap_or_default().to_string_lossy().into_owned();
            std::fs::copy(&from, dir.join(&to_name)).ok().map(|_| VisionFile { file: to_name, url: v.url.clone(), size: v.size, sha256: v.sha256.clone() })
        }
        _ => None,
    };
    let result = register(state, &id, &file, spec, &o, &quant, vision);
    if let Err(e) = result {
        std::fs::remove_dir_all(&dir).ok();
        return Err(e);
    }
    std::fs::remove_dir_all(&work).ok();
    Ok(id)
}

/// Adds the copy to the models list, as a model of its own (removing it
/// deletes its folder).
fn register(state: &AppState, id: &str, file: &Path, original: &crate::catalog::ModelSpec, o: &Original, quant: &str, vision: Option<VisionFile>) -> Result<(), String> {
    let mut spec = crate::local::read_spec(file).map_err(|e| format!("The copy came out unreadable ({e})."))?;
    let (refusals, base, of) = JOB.lock().unwrap().as_ref().map_or((None, None, 0), |(j, _)| (j.refusals, j.base_refusals, j.of));
    let tested = match (refusals, base) {
        (Some(r), Some(b)) => format!(" It refused {r} of {of} test requests; the original refused {b}."),
        _ => String::new(),
    };
    spec.id = id.to_string();
    spec.name = copy_name(&original.name);
    spec.publisher = "Made on this PC".into();
    spec.source = format!("{SITE}/{}", o.repo);
    spec.description = format!("An uncensored copy of {}, made on this PC with Heretic from {}.{tested} It answers what the original refused, so check what it says. Removing it deletes the file.", original.name, o.repo);
    spec.license = crate::catalog::License {
        name: o.license.clone().unwrap_or_else(|| "The original's license".into()),
        url: format!("{SITE}/{}", o.repo),
        commercial: false,
        note: Some("A changed copy of the original model: its license still applies.".into()),
    };
    spec.ratings = original.ratings.clone();
    spec.tags = if vision.is_some() { vec!["vision".into()] } else { Vec::new() };
    spec.vision = vision;
    spec.local = None;
    let size = spec.variants.first().map_or(0, |v| v.size);
    let conn = state.db.lock().unwrap();
    crate::hub::remember_made(&conn, &spec)?;
    db::save_installed(&conn, &InstalledModel { model_id: id.to_string(), quant: quant.to_string(), path: file.display().to_string(), size, installed_at: db::now_ms(), tps: None, prompt_tps: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_lines_say_how_far_it_is() {
        assert_eq!(said("SULCUS stage directions"), Some(Said::Stage("loading")));
        assert_eq!(said("SULCUS stage saving\r"), Some(Said::Stage("saving")));
        assert_eq!(said("SULCUS base 91 100"), Some(Said::Base(91, 100)));
        assert_eq!(said("SULCUS trial 3 200"), Some(Said::Trial(3, 200)));
        assert_eq!(said("SULCUS tried 13 0.0337"), Some(Said::Tried(13, 0.0337)));
        assert_eq!(said("SULCUS chose 8 7 0.0661"), Some(Said::Chose(8, 7, 0.0661)));
        assert_eq!(said("SULCUS saved"), Some(Said::Saved));
        assert_eq!(said("Running trial 3 of 200..."), None);
        assert_eq!(said("SULCUS stage nothing"), None);
    }

    #[test]
    fn the_original_is_the_model_its_gguf_was_made_from() {
        let gguf = json!({ "tags": ["gguf", "base_model:Qwen/Qwen3-4B-Instruct-2507", "base_model:quantized:Qwen/Qwen3-4B-Instruct-2507"], "cardData": { "base_model": "Qwen/Qwen3-4B-Base" } });
        assert_eq!(original_in(&gguf).as_deref(), Some("Qwen/Qwen3-4B-Instruct-2507"));
        let card_only = json!({ "tags": ["gguf"], "cardData": { "base_model": ["google/gemma-3-4b-it"] } });
        assert_eq!(original_in(&card_only).as_deref(), Some("google/gemma-3-4b-it"));
        assert_eq!(original_in(&json!({ "tags": ["gguf"], "cardData": { "base_model": ["a/b", "c/d"] } })), None, "a merge of several has no one original");
    }

    fn f(path: &str, size: u64) -> OriginalFile {
        OriginalFile { path: path.into(), size, sha256: path.ends_with(".safetensors").then(|| "x".into()) }
    }

    #[test]
    fn only_the_weights_and_settings_are_downloaded() {
        let files = vec![
            f("config.json", 1),
            f("tokenizer.json", 2),
            f("tokenizer_config.json", 3),
            f("chat_template.jinja", 4),
            f("README.md", 5),
            f("LICENSE", 6),
            f(".gitattributes", 7),
            f("model.safetensors", 1000),
            f("original/consolidated.pth", 9000),
            f("onnx/model.onnx", 9000),
        ];
        let got: Vec<String> = wanted(&files, None).into_iter().map(|f| f.path).collect();
        assert_eq!(got, ["config.json", "tokenizer.json", "tokenizer_config.json", "chat_template.jinja", "model.safetensors"]);
    }

    #[test]
    fn split_weights_come_from_the_index() {
        // Mistral's uploads also carry the same weights in its own layout.
        let files = vec![f("config.json", 1), f("model.safetensors.index.json", 1), f("model-00001-of-00002.safetensors", 500), f("model-00002-of-00002.safetensors", 500), f("consolidated.safetensors", 1000), f("params.json", 1), f("tekken.json", 1)];
        let index = json!({ "weight_map": { "a": "model-00001-of-00002.safetensors", "b": "model-00002-of-00002.safetensors", "c": "model-00002-of-00002.safetensors" } });
        let got = wanted(&files, Some(&index));
        assert!(!got.iter().any(|f| f.path == "consolidated.safetensors"));
        assert_eq!(weights_size(&got), 1000);
        assert!(got.iter().any(|f| f.path == "model.safetensors.index.json") && got.iter().any(|f| f.path == "tekken.json"));
    }

    #[test]
    fn compressed_and_self_loading_originals_are_turned_away() {
        assert!(config_problem(&json!({ "architectures": ["Qwen2ForCausalLM"] })).is_none());
        assert!(config_problem(&json!({ "quantization_config": { "quant_method": "mxfp4" } })).is_none(), "gpt-oss comes in MXFP4, which Heretic loads");
        assert!(config_problem(&json!({ "quantization_config": { "quant_method": "fp8" } })).unwrap().contains("fp8"));
        assert!(config_problem(&json!({ "auto_map": { "AutoModelForCausalLM": "modeling_x.X" } })).is_some());
    }

    const GB: u64 = 1 << 30;

    #[test]
    fn big_models_are_squeezed_to_fit_the_card() {
        // His RTX 5070 (12 GB) and 32 GB of memory.
        let (card, ram) = (12 * GB, 32 * GB);
        assert_eq!(plan(GB, card, ram), Ok(false), "a 0.5B fits as it is");
        assert_eq!(plan(8 * GB, card, ram), Ok(false), "a 4B (8 GB) fits as it is");
        assert_eq!(plan(16 * GB, card, ram), Ok(true), "an 8B goes in 4 bits");
        assert!(plan(28 * GB, card, ram).unwrap_err().contains("memory"), "a 14B fits the card in 4 bits, but saving needs more memory than 32 GB");
        assert!(plan(54 * GB, card, 128 * GB).unwrap_err().contains("too big"));
    }

    #[test]
    fn quantizations_that_need_a_sample_become_q4_k_m() {
        assert_eq!(target_quant("Q4_K_M"), "Q4_K_M");
        assert_eq!(target_quant("q8_0"), "Q8_0");
        assert_eq!(target_quant("IQ4_XS"), "Q4_K_M");
        assert_eq!(target_quant("Unknown"), "Q4_K_M");
        // unsloth's dynamic kinds keep their size, not Q4_K_M.
        assert_eq!(target_quant("UD-Q6_K_XL"), "Q6_K");
        assert_eq!(target_quant("UD-Q5_K_XL"), "Q5_K_M");
        assert_eq!(target_quant("Q8_K_XL"), "Q8_0");
        assert_eq!(target_quant("UD-Q3_K_XL"), "Q3_K_L");
        assert_eq!(target_quant("UD-Q4_K_M"), "Q4_K_M");
    }

    /// A copy still refusing most of what the original did isn't kept; one
    /// that took most out is, and a model that hardly refused isn't judged.
    #[test]
    fn copies_that_took_too_little_out_are_turned_away() {
        assert!(too_little(Some(90), Some(80)));
        assert!(!too_little(Some(90), Some(12)));
        assert!(!too_little(Some(8), Some(8)));
        assert!(!too_little(None, Some(50)));
    }

    #[test]
    fn it_runs_from_its_folder_with_its_files() {
        let a = args(Path::new(r"C:\e\heretic-1.4.0-b11450"), Path::new(r"C:\w\original"), Path::new(r"C:\w\saved"), Path::new(r"C:\w\heretic"), Path::new(r"C:\e\heretic-1.4.0-b11450\prompts"), 1000, true);
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(&a[..4], ["run", "--frozen", "--no-sync", "--project"]);
        assert!(a.contains(&r"C:\e\heretic-1.4.0-b11450\uncensor.py".to_string()));
        assert!(a.windows(2).any(|w| w == ["--trials", "400"]), "tries are kept to 400 at most");
        assert_eq!(a.last().map(String::as_str), Some("--quantize"));
        let c = convert_args(Path::new(r"C:\e\h"), Path::new(r"C:\w\saved"), Path::new(r"C:\w\copy.gguf"), "Qwen3 4B (uncensored)");
        let c: Vec<String> = c.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(c.iter().any(|s| s.ends_with(r"llama-convert\convert_hf_to_gguf.py")));
        assert_eq!(&c[c.len() - 2..], ["--model-name", "Qwen3 4B (uncensored)"], "named, not after its folder");
    }

    #[test]
    fn estimates_grow_with_the_model_and_the_tries() {
        assert!(estimate(GB, QUICK, false) < estimate(8 * GB, QUICK, false));
        assert!(estimate(8 * GB, QUICK, false) < estimate(8 * GB, THOROUGH, false));
        assert!(estimate(8 * GB, QUICK, false) < estimate(8 * GB, QUICK, true));
    }

    #[test]
    fn download_speed_is_measured_each_second() {
        use std::time::Duration;
        let t = Instant::now();
        let mut s = Speed::default();
        assert_eq!(s.add(5_000_000, t), None, "the first report only starts the clock");
        assert_eq!(s.add(20_000_000, t + Duration::from_millis(500)), None, "too soon");
        assert_eq!(s.add(55_000_000, t + Duration::from_secs(1)), Some(50_000_000.0));
        assert_eq!(s.add(80_000_000, t + Duration::from_secs(3)), Some(12_500_000.0));
    }
}

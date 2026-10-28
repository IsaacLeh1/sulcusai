// SPDX-License-Identifier: AGPL-3.0-only
//! Natural voices: Supertonic 3 (31 languages) on ONNX Runtime.
//!
//! Text goes in as plain characters (no phonemizer, so no GPL espeak), and
//! four small networks turn it into speech: a duration predictor, a text
//! encoder, a few flow-matching steps, and a vocoder. ONNX Runtime is a
//! downloaded, hash-checked DLL loaded at run time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use unicode_normalization::UnicodeNormalization;

use crate::catalog::{EngineSpec, License};
use crate::paths::Paths;
use crate::tts::Speech;
use crate::{db, download, engine, net, AppState, AppStateRef};

/// Flow-matching steps: 5 is the quality setting the model was tuned for.
const STEPS: usize = 5;
/// Unload after this long unused (the models take ~500 MB of memory).
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceCatalog {
    pub runtime: EngineSpec,
    pub runtime_dll: String,
    /// ONNX Runtime for graphics cards, for what runs much faster there (a
    /// song's parts, a voice sung again).
    #[serde(default)]
    pub cards: Option<CardRuntime>,
    pub models: Vec<VoicePack>,
}

/// ONNX Runtime for graphics cards (64-bit Windows): DirectML's build, which
/// runs on any DirectX 12 card (AMD's, Intel's, NVIDIA's older ones) and on
/// the processor like the plain build; and for NVIDIA cards CUDA 13 runs
/// on, the CUDA part of the same version with NVIDIA's libraries beside it,
/// used first there (a little faster). One build has both, so the CUDA part
/// can come later without SulcusAI starting again.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CardRuntime {
    pub name: String,
    pub license: String,
    pub build: String,
    /// DirectML's build of ONNX Runtime and DirectML (NuGet packages).
    pub base: Vec<Part>,
    /// The CUDA part and NVIDIA's libraries (Python wheels).
    pub cuda: Vec<Part>,
    /// The oldest NVIDIA driver its CUDA runs on, and the oldest card
    /// (compute capability).
    pub min_driver: f64,
    pub min_compute: f64,
    /// Free memory the card needs (MiB): `CARD_ARENA` for the work and the
    /// rest for CUDA itself.
    pub min_free_mib: u64,
    /// Folders of earlier builds for NVIDIA cards, whose NVIDIA libraries
    /// are linked rather than downloaded again.
    #[serde(default)]
    pub earlier: Vec<String>,
}

/// A downloaded archive (zips all) and the DLLs kept from it: those whose
/// path in it contains one of `keep`, or every one when it's empty.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Part {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub keep: Vec<String>,
    /// NVIDIA's own library, which an earlier build may already have.
    #[serde(default)]
    pub nvidia: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackFile {
    pub path: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StyleInfo {
    pub id: String,
    pub name: String,
    pub gender: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoicePack {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub source: String,
    pub description: String,
    pub license: License,
    pub languages: Vec<String>,
    pub files: Vec<PackFile>,
    pub styles: Vec<StyleInfo>,
}

impl VoicePack {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Where an installed pack and the runtime are, set at startup and install.
#[derive(Debug, Clone)]
pub struct Installed {
    pub dll: PathBuf,
    pub dir: PathBuf,
    pub pack: VoicePack,
}

fn installed_slot() -> &'static Mutex<Option<Installed>> {
    static S: OnceLock<Mutex<Option<Installed>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

pub fn installed() -> Option<Installed> {
    installed_slot().lock().unwrap().clone()
}

fn runtime_dir(c: &VoiceCatalog) -> String {
    format!("onnxruntime-{}-{}", c.runtime.build, engine::cpu_key())
}

/// The library's file name on this system.
fn runtime_file(c: &VoiceCatalog) -> String {
    if cfg!(windows) {
        c.runtime_dll.clone()
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib".into()
    } else {
        "libonnxruntime.so".into()
    }
}

/// The downloaded ONNX Runtime library, if present: the one for graphics
/// cards when that's here (it runs everything else on the processor just
/// the same, and a process loads only one).
pub fn runtime_dll(paths: &Paths, c: &VoiceCatalog) -> Option<PathBuf> {
    card_runtime_dll(paths, c).or_else(|| engine::find_exe(&paths.engines.join(runtime_dir(c)), &runtime_file(c)))
}

/// Downloads and unpacks ONNX Runtime if it isn't there yet: on a PC with a
/// graphics card, the build for cards (so the first use doesn't load the
/// processor's, which would keep the card unused until a restart).
pub async fn ensure_runtime(state: &Arc<AppState>, cancel: &AtomicBool, emit: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<PathBuf, String> {
    let c = &state.catalog.voices;
    if let Some(dll) = card_runtime_dll(&state.paths, c) {
        return Ok(dll);
    }
    if card_here(state) && !loaded_for_processor() {
        state.log("network", "Downloading ONNX Runtime for graphics cards (DirectML)");
        return ensure_card_runtime(state, false, cancel, &|r, t| emit("engine", r, t)).await;
    }
    let asset = c.runtime.assets.get(engine::cpu_key()).ok_or("Natural voices aren't available on this system yet (no ONNX Runtime build for it).")?;
    let rdir = runtime_dir(c);
    if runtime_dll(&state.paths, c).is_none() {
        state.log("network", &format!("Downloading ONNX Runtime from {}", crate::host_of(&asset.url)));
    }
    engine::ensure_unpacked(&state.paths, &rdir, asset, &runtime_file(c), cancel, |r, t| emit("engine", r, t)).await
}

/// Finds an installed pack (every file present) and its runtime.
pub fn detect(paths: &Paths, c: &VoiceCatalog) -> Option<Installed> {
    let dll = runtime_dll(paths, c)?;
    let pack = c.models.first()?;
    let dir = paths.models.join(&pack.id);
    pack.files.iter().all(|f| dir.join(&f.path).metadata().is_ok_and(|m| m.len() == f.size)).then(|| Installed {
        dll,
        dir,
        pack: pack.clone(),
    })
}

pub fn refresh(paths: &Paths, c: &VoiceCatalog) {
    *installed_slot().lock().unwrap() = detect(paths, c);
}

// ---------- text ----------

/// Cleans text the way the model was trained to read it.
pub fn prepare(text: &str, lang: &str) -> String {
    let mut t: String = text.nfkd().collect();
    t.retain(|c| !is_emoji(c));
    for (from, to) in [
        ("–", "-"), ("‑", "-"), ("\u{2014}", "-"), ("_", " "), ("\u{201c}", "\""), ("\u{201d}", "\""),
        ("\u{2018}", "'"), ("\u{2019}", "'"), ("´", "'"), ("`", "'"), ("[", " "), ("]", " "),
        ("|", " "), ("/", " "), ("#", " "), ("→", " "), ("←", " "), ("@", " at "),
        ("e.g.,", "for example, "), ("i.e.,", "that is, "),
    ] {
        t = t.replace(from, to);
    }
    t.retain(|c| !"♥☆♡©\\".contains(c));
    for p in [",", ".", "!", "?", ";", ":", "'"] {
        t = t.replace(&format!(" {p}"), p);
    }
    while t.contains("\"\"") {
        t = t.replace("\"\"", "\"");
    }
    while t.contains("''") {
        t = t.replace("''", "'");
    }
    let mut t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if !t.ends_with(|c: char| ".!?;:,'\")]}…。」』】〉》›»".contains(c)) {
        t.push('.');
    }
    format!("<{lang}>{t}</{lang}>")
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F1E6..=0x1F1FF)
}

/// Splits long text into pieces the model reads well, at sentence ends.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        let mut cur = String::new();
        let mut sentences = Vec::new();
        let mut start = 0;
        let b = para.as_bytes();
        for i in 0..b.len() {
            if matches!(b[i], b'.' | b'!' | b'?') && b.get(i + 1) == Some(&b' ') {
                sentences.push(&para[start..=i]);
                start = i + 2;
            }
        }
        if start < para.len() {
            sentences.push(&para[start..]);
        }
        for s in sentences {
            if !cur.is_empty() && cur.len() + s.len() + 1 > max {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(s);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

// ---------- the engine ----------

struct Style {
    ttl: (Vec<i64>, Vec<f32>),
    dp: (Vec<i64>, Vec<f32>),
}

struct Engine {
    dp: Session,
    enc: Session,
    est: Session,
    voc: Session,
    indexer: Vec<i64>,
    sample_rate: u32,
    chunk: usize,
    latent_dim: usize,
    styles: HashMap<String, Style>,
    dir: PathBuf,
    last_used: Instant,
}

fn engine_slot() -> &'static Mutex<Option<Engine>> {
    static S: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The ONNX Runtime library this process loaded (only one can be).
static LOADED: OnceLock<PathBuf> = OnceLock::new();

pub fn runtime_ready(dll: &Path) -> Result<(), String> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        // The build for cards finds CUDA and cuDNN beside it (they may come
        // later, while it runs), and cuDNN loads its parts by name: its
        // folder goes first on the path. Cards are numbered as nvidia-smi
        // does (`pick_card`). DirectML comes from beside it too, not the
        // older one in System32 (a library already loaded is the one used).
        #[cfg(windows)]
        if dll.with_file_name(DML_LIB).is_file() {
            use windows::core::HSTRING;
            use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_WITH_ALTERED_SEARCH_PATH};
            if let Err(e) = unsafe { LoadLibraryExW(&HSTRING::from(dll.with_file_name(DML_LIB).as_os_str()), None, LOAD_WITH_ALTERED_SEARCH_PATH) } {
                crate::applog::line("media", &format!("Couldn't load DirectML ({e}); graphics cards other than NVIDIA's won't be used"));
            }
        }
        if cfg!(windows) && (dll.with_file_name(CUDA_PROVIDER).is_file() || dll.with_file_name(DML_LIB).is_file()) {
            if let Some(dir) = dll.parent() {
                let path = std::env::var_os("PATH").unwrap_or_default();
                let dirs = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&path));
                if let Ok(joined) = std::env::join_paths(dirs) {
                    std::env::set_var("PATH", joined);
                }
                std::env::set_var("CUDA_DEVICE_ORDER", "PCI_BUS_ID");
            }
        }
        let builder = ort::init_from(dll).map_err(|e| format!("Couldn't start the natural voices' speech engine (ONNX Runtime): {e}"))?;
        builder.commit();
        let _ = LOADED.set(dll.to_path_buf());
        Ok(())
    })
    .clone()
}

// ---------- on graphics cards ----------

/// ONNX Runtime's CUDA part (NVIDIA cards).
const CUDA_PROVIDER: &str = "onnxruntime_providers_cuda.dll";
/// DirectML, beside the build that has it (any DirectX 12 card).
const DML_LIB: &str = "DirectML.dll";
/// Written when DirectML's build is in, and when the CUDA part and NVIDIA's
/// libraries are.
const BASE_DONE: &str = ".base-complete";
const CUDA_DONE: &str = ".cuda-complete";
/// The most a model's work may take on the card: unbounded, HTDemucs took
/// 7.4 GB; in 3 it ran as fast, leaving room for a chat model (in 2 it
/// failed).
const CARD_ARENA: usize = 3 << 30;

/// The build for graphics cards, for this system, if there is one.
fn card_parts(c: &VoiceCatalog) -> Option<&CardRuntime> {
    c.cards.as_ref().filter(|_| cfg!(all(windows, target_arch = "x86_64")))
}

fn card_dir(paths: &Paths, r: &CardRuntime) -> PathBuf {
    paths.engines.join(format!("onnxruntime-{}-cards", r.build))
}

fn part_mark(p: &Part) -> String {
    format!(".part-{}", &p.sha256[..16])
}

/// The downloaded build for graphics cards, once DirectML's part is in.
pub fn card_runtime_dll(paths: &Paths, c: &VoiceCatalog) -> Option<PathBuf> {
    let dir = card_dir(paths, card_parts(c)?);
    dir.join(BASE_DONE).is_file().then(|| dir.join(&c.runtime_dll)).filter(|d| d.is_file())
}

/// The CUDA part and NVIDIA's libraries are in too.
pub fn cuda_ready(paths: &Paths, c: &VoiceCatalog) -> bool {
    card_parts(c).is_some_and(|r| card_dir(paths, r).join(CUDA_DONE).is_file())
}

/// A complete earlier build for NVIDIA cards, whose NVIDIA libraries are
/// linked rather than downloaded again.
fn earlier_cuda(paths: &Paths, r: &CardRuntime) -> Option<PathBuf> {
    r.earlier.iter().map(|n| paths.engines.join(n)).find(|d| d.join(".complete").is_file())
}

/// Earlier builds for NVIDIA cards that aren't needed any more: the cards'
/// CUDA part is complete, with their libraries linked into it.
pub fn earlier_builds(paths: &Paths, c: &VoiceCatalog) -> Vec<PathBuf> {
    match card_parts(c) {
        Some(r) if cuda_ready(paths, c) => r.earlier.iter().map(|n| paths.engines.join(n)).filter(|d| d.is_dir() && !LOADED.get().is_some_and(|l| l.starts_with(d))).collect(),
        _ => Vec::new(),
    }
}

/// The parts still to download (with the CUDA part, or not).
fn missing<'a>(paths: &Paths, r: &'a CardRuntime, cuda: bool) -> Vec<&'a Part> {
    let dir = card_dir(paths, r);
    let earlier = cuda && earlier_cuda(paths, r).is_some();
    let extra: &[Part] = if cuda { &r.cuda } else { &[] };
    r.base.iter().chain(extra).filter(|p| !dir.join(part_mark(p)).is_file() && !(p.nvidia && earlier)).collect()
}

/// What's still to download for the cards, in bytes.
pub fn card_runtime_size(paths: &Paths, c: &VoiceCatalog, cuda: bool) -> u64 {
    card_parts(c).map_or(0, |r| missing(paths, r, cuda).iter().map(|p| p.size).sum())
}

/// Downloads the build for graphics cards: DirectML's part, and with `cuda`
/// the CUDA part and NVIDIA's libraries (linked from an earlier build when
/// one is here). Each part is kept as it's done, so a stopped download goes
/// on from the next. Engine downloads are user-started installs, allowed at
/// every level.
pub async fn ensure_card_runtime(state: &Arc<AppState>, cuda: bool, cancel: &AtomicBool, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<PathBuf, String> {
    let c = &state.catalog.voices;
    let r = card_parts(c).ok_or("There's no ONNX Runtime for graphics cards on this system.")?;
    let dir = card_dir(&state.paths, r);
    // One download at a time: a job and the background fetch may both ask.
    static FETCHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = FETCHING.lock().await;
    if dir.join(BASE_DONE).is_file() && (!cuda || dir.join(CUDA_DONE).is_file()) {
        return card_runtime_dll(&state.paths, c).ok_or_else(|| format!("{} is missing from the graphics cards' ONNX Runtime.", c.runtime_dll));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let earlier = if cuda { earlier_cuda(&state.paths, r) } else { None };
    if let Some(from) = earlier.clone() {
        let into = dir.clone();
        tokio::task::spawn_blocking(move || link_libraries(&from, &into)).await.map_err(|e| e.to_string())??;
    }
    let parts = missing(&state.paths, r, cuda);
    let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false)?;
    let total: u64 = parts.iter().map(|p| p.size).sum();
    let mut done = 0;
    for (i, p) in parts.iter().enumerate() {
        state.log("network", &format!("Downloading {} ({}) from {}", p.url.rsplit('/').next().unwrap_or(""), crate::size_label(p.size), crate::host_of(&p.url)));
        let archive = state.paths.engines.join(format!("onnxruntime-{}-cards-{i}.zip", r.build));
        download::fetch_verified(&client, &p.url, &archive, p.size, &p.sha256, cancel, |got, _| progress(done + got, total)).await?;
        let (from, into, keep) = (archive.clone(), dir.clone(), p.keep.clone());
        tokio::task::spawn_blocking(move || unzip_kept(&from, &into, &keep)).await.map_err(|e| e.to_string())??;
        std::fs::remove_file(&archive).ok();
        std::fs::write(dir.join(part_mark(p)), b"").map_err(|e| e.to_string())?;
        done += p.size;
        progress(done, total);
    }
    if !dir.join(DML_LIB).is_file() || !dir.join(&c.runtime_dll).is_file() {
        return Err(format!("The graphics cards' ONNX Runtime download didn't contain {DML_LIB} and {}.", c.runtime_dll));
    }
    std::fs::write(dir.join(BASE_DONE), r.build.as_bytes()).map_err(|e| e.to_string())?;
    if cuda {
        if !dir.join(CUDA_PROVIDER).is_file() {
            return Err(format!("The graphics cards' ONNX Runtime download didn't contain {CUDA_PROVIDER}."));
        }
        // The earlier build's folder goes with the cleanup of old engines
        // (`earlier_builds`): its libraries are linked here, so little room
        // is freed, and an older SulcusAI may still be using it.
        std::fs::write(dir.join(CUDA_DONE), r.build.as_bytes()).map_err(|e| e.to_string())?;
    }
    card_runtime_dll(&state.paths, c).ok_or_else(|| format!("{} is missing from the graphics cards' ONNX Runtime.", c.runtime_dll))
}

/// NVIDIA's libraries from an earlier build, linked into `into` (copied
/// where a link can't be made); not that build's own ONNX Runtime.
fn link_libraries(from: &Path, into: &Path) -> Result<(), String> {
    for e in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !name.ends_with(".dll") || name.starts_with("onnxruntime") || into.join(e.file_name()).is_file() {
            continue;
        }
        if std::fs::hard_link(e.path(), into.join(e.file_name())).is_err() {
            std::fs::copy(e.path(), into.join(e.file_name())).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

/// The DLLs of an archive whose path in it contains one of `keep` (every
/// DLL when it's empty), side by side in `into`.
fn unzip_kept(archive: &Path, into: &Path, keep: &[String]) -> Result<(), String> {
    if keep.is_empty() {
        return engine::unzip_libraries(archive, into);
    }
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("Couldn't unpack {}: {e}", archive.display()))?;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        let path = e.name().replace('\\', "/");
        let Some(file) = path.rsplit('/').next().filter(|f| f.to_lowercase().ends_with(".dll")).map(str::to_string) else { continue };
        if e.is_dir() || !keep.iter().any(|k| path.contains(k.as_str())) {
            continue;
        }
        let mut out = std::fs::File::create(into.join(&file)).map_err(|e| e.to_string())?;
        std::io::copy(&mut e, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whether this process's ONNX Runtime can use graphics cards (it can't
/// change until SulcusAI starts again).
pub fn loaded_for_cards() -> bool {
    loaded_for_cuda() || loaded_for_dml()
}

fn loaded_for_cuda() -> bool {
    LOADED.get().is_some_and(|d| d.with_file_name(CUDA_PROVIDER).is_file())
}

fn loaded_for_dml() -> bool {
    LOADED.get().is_some_and(|d| d.with_file_name(DML_LIB).is_file())
}

/// This process loaded the processor's ONNX Runtime (before the cards' was
/// downloaded): the cards' is used from SulcusAI's next start.
pub fn loaded_for_processor() -> bool {
    LOADED.get().is_some() && !loaded_for_cards()
}

/// The PC's main card is NVIDIA's (training needs one).
pub fn nvidia_here(state: &AppState) -> bool {
    state.hardware.read().unwrap().best_gpu().is_some_and(|g| g.vendor == "NVIDIA")
}

/// The PC has a dedicated card ONNX Runtime's work can go to, and there's a
/// build for cards on this system (`pick_card` checks the rest when it's time).
pub fn card_here(state: &AppState) -> bool {
    card_parts(&state.catalog.voices).is_some() && state.hardware.read().unwrap().best_gpu().is_some()
}

/// A card ONNX Runtime can use: an NVIDIA card through CUDA (its number as
/// nvidia-smi counts), or any DirectX 12 card through DirectML (its number
/// as DXGI counts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Card {
    pub device: i32,
    pub dml: bool,
}

/// The card picked for ONNX Runtime's work: which, how, and whether it's the
/// second card.
#[derive(Debug, Clone)]
pub struct Picked {
    pub card: Card,
    pub name: String,
    pub second: bool,
    /// CUDA, whose part isn't downloaded yet (`ensure_card_runtime`).
    pub needs_cuda: bool,
}

/// An NVIDIA card as nvidia-smi reports it.
#[derive(Debug, Clone, Default)]
pub struct Smi {
    pub index: i32,
    pub name: String,
    pub driver: String,
    pub compute: f64,
    pub used_mib: f64,
    pub total_mib: f64,
}

/// The NVIDIA cards (empty when nvidia-smi isn't there or doesn't answer).
pub async fn nvidia_smi() -> Vec<Smi> {
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=index,name,driver_version,compute_cap,memory.used,memory.total", "--format=csv,noheader,nounits"]).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(5), cmd.output()).await else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.split(',').map(|f| f.trim().to_string()).collect::<Vec<_>>())
        .filter(|f| f.len() == 6)
        .map(|f| {
            let num = |i: usize| f[i].parse::<f64>().unwrap_or(0.0);
            Smi { index: f[0].parse().unwrap_or(0), name: f[1].clone(), driver: f[2].clone(), compute: num(3), used_mib: num(4), total_mib: num(5) }
        })
        .collect()
}

/// The card for ONNX Runtime's work now, or why none can take it. With
/// `second`, the second card first (the main one is busy). On an NVIDIA
/// card CUDA 13 runs on (an RTX 20 or newer, driver 580 or newer), CUDA
/// (`needs_cuda` when its part is still to download: DirectML ran out of
/// memory taking a song apart on a 12 GB card where CUDA, capped at 3 GB,
/// didn't); on every other DirectX 12 card (AMD's, Intel's, NVIDIA's older
/// ones), DirectML. SULCUSAI_ONNX_CPU=1 keeps work on the processor (to
/// compare).
pub async fn pick_card(state: &Arc<AppState>, second: bool) -> Result<Picked, String> {
    if std::env::var_os("SULCUSAI_ONNX_CPU").is_some() {
        return Err("SULCUSAI_ONNX_CPU is set (for comparing)".into());
    }
    let r = card_parts(&state.catalog.voices).ok_or("there's no ONNX Runtime for graphics cards on this system")?.clone();
    let (main, other) = {
        let hw = state.hardware.read().unwrap();
        (hw.best_gpu().cloned(), hw.second_gpu().cloned())
    };
    let Some(main) = main else { return Err(no_card(state)) };
    let mut order = vec![(main, false)];
    if let Some(o) = other {
        if second {
            order.insert(0, (o, true));
        } else {
            order.push((o, true));
        }
    }
    let smi = if order.iter().any(|(g, _)| g.vendor == "NVIDIA") { nvidia_smi().await } else { Vec::new() };
    let mut why = Vec::new();
    for (g, is_second) in order {
        if let Some(s) = smi.iter().find(|s| crate::hardware::same_name(&s.name, &g.name)) {
            let free = s.total_mib - s.used_mib;
            if free < r.min_free_mib as f64 {
                why.push(format!("{} has {:.1} GB of its memory free and this needs {:.1} GB", g.name, free / 1024.0, r.min_free_mib as f64 / 1024.0));
                continue;
            }
            let driver = s.driver.parse::<f64>().unwrap_or(0.0);
            // SULCUSAI_ONNX_DML=1: DirectML, as on a card CUDA can't use (to compare).
            if std::env::var_os("SULCUSAI_ONNX_DML").is_some() {
            } else if driver >= r.min_driver && s.compute >= r.min_compute {
                let needs_cuda = !cuda_ready(&state.paths, &state.catalog.voices);
                return Ok(Picked { card: Card { device: s.index, dml: false }, name: g.name.clone(), second: is_second, needs_cuda });
            } else if driver < r.min_driver {
                once(&format!("CUDA isn't used on {}: its driver ({}) is older than {} (DirectML runs the work on it instead, a little slower; updating the driver lets CUDA run)", g.name, s.driver, r.min_driver));
            } else {
                once(&format!("{} is older than CUDA 13 supports: DirectML runs the work on it instead", g.name));
            }
        }
        match g.dxgi {
            Some(i) => return Ok(Picked { card: Card { device: i as i32, dml: true }, name: g.name.clone(), second: is_second, needs_cuda: false }),
            None => why.push(format!("DirectX didn't list {}", g.name)),
        }
    }
    Err(why.join("; "))
}

/// A line in the app log once per run.
fn once(line: &str) {
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap();
    if !seen.iter().any(|s| s == line) {
        seen.push(line.to_string());
        crate::applog::line("media", line);
    }
}

/// Why there's no card to use: none found, one that isn't working (no
/// driver, stopped), or only graphics built into the processor.
pub fn no_card(state: &AppState) -> String {
    let hw = state.hardware.read().unwrap();
    if let Some(b) = hw.broken_cards.first() {
        let what = match b.code {
            22 => "turned off in Device Manager",
            28 => "missing its driver",
            43 => "stopped by Windows after a problem (code 43)",
            _ => "running on Windows' stand-in driver",
        };
        return format!("Windows lists {} but it's {what}: install or update its driver", b.name);
    }
    match hw.gpus.iter().find(|g| g.integrated) {
        Some(g) => format!("this PC has only the graphics built into the processor ({}), which isn't used for this", g.name),
        None => "no graphics card was found".into(),
    }
}


/// A model opened on the NVIDIA card (CUDA, with `CARD_ARENA`) when one is
/// given and this process's ONNX Runtime can use it, else on the processor;
/// and whether it's on the card.
pub fn session_on(model: &Path, threads: usize, card: Option<Card>) -> Result<(Session, bool), String> {
    session_within(model, threads, card, CARD_ARENA)
}

/// `session_on` with another most it may take on the card (bytes).
pub fn session_within(model: &Path, threads: usize, card: Option<Card>, arena: usize) -> Result<(Session, bool), String> {
    let fail = |err: ort::Error| format!("Couldn't open {}: {err}", model.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()));
    if let Some(c) = card.filter(|c| c.dml && loaded_for_dml()) {
        // DirectML wants these two off.
        let dml = ort::ep::DirectML::default().with_device_id(c.device).build().error_on_failure();
        let built = Session::builder()
            .and_then(|b| b.with_memory_pattern(false).map_err(Into::into))
            .and_then(|b| b.with_parallel_execution(false).map_err(Into::into))
            .and_then(|b| b.with_execution_providers([dml]).map_err(Into::into))
            .and_then(|mut b| b.commit_from_file(model));
        match built {
            Ok(s) => return Ok((s, true)),
            Err(err) => crate::applog::line("media", &format!("DirectML couldn't open {} on the card ({err}); using the processor", model.display())),
        }
    }
    if let Some(c) = card.filter(|c| !c.dml && loaded_for_cuda()) {
        let cuda = ort::ep::CUDA::default()
            .with_device_id(c.device)
            .with_conv_algorithm_search(ort::ep::cuda::ConvAlgorithmSearch::Heuristic)
            .with_arena_extend_strategy(ort::ep::ArenaExtendStrategy::SameAsRequested)
            .with_memory_limit(arena)
            .build()
            .error_on_failure();
        match Session::builder().and_then(|b| b.with_execution_providers([cuda]).map_err(Into::into)).and_then(|mut b| b.commit_from_file(model)) {
            Ok(s) => return Ok((s, true)),
            Err(err) => crate::applog::line("media", &format!("The graphics card couldn't open {} ({err}); using the processor", model.display())),
        }
    }
    let s = Session::builder().map_err(fail)?.with_intra_threads(threads.max(1)).map_err(|err| fail(err.into()))?.commit_from_file(model).map_err(fail)?;
    Ok((s, false))
}

fn e(err: impl std::fmt::Display) -> String {
    format!("The natural voice failed: {err}")
}

impl Engine {
    fn load(i: &Installed) -> Result<Engine, String> {
        runtime_ready(&i.dll)?;
        let threads = std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).clamp(2, 8));
        let open = |name: &str| -> Result<Session, String> {
            Session::builder()
                .map_err(e)?
                .with_intra_threads(threads)
                .map_err(e)?
                .commit_from_file(i.dir.join("onnx").join(name))
                .map_err(e)
        };
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(i.dir.join("onnx/tts.json")).map_err(e)?).map_err(e)?;
        let indexer: Vec<i64> = serde_json::from_str(&std::fs::read_to_string(i.dir.join("onnx/unicode_indexer.json")).map_err(e)?).map_err(e)?;
        let ccf = cfg["ttl"]["chunk_compress_factor"].as_u64().unwrap_or(6) as usize;
        Ok(Engine {
            dp: open("duration_predictor.onnx")?,
            enc: open("text_encoder.onnx")?,
            est: open("vector_estimator.onnx")?,
            voc: open("vocoder.onnx")?,
            indexer,
            sample_rate: cfg["ae"]["sample_rate"].as_u64().unwrap_or(44_100) as u32,
            chunk: cfg["ae"]["base_chunk_size"].as_u64().unwrap_or(512) as usize * ccf,
            latent_dim: cfg["ttl"]["latent_dim"].as_u64().unwrap_or(24) as usize * ccf,
            styles: HashMap::new(),
            dir: i.dir.clone(),
            last_used: Instant::now(),
        })
    }

    fn style(&mut self, id: &str) -> Result<&Style, String> {
        if !self.styles.contains_key(id) {
            if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err("Unknown voice.".into());
            }
            let v: Value = serde_json::from_str(
                &std::fs::read_to_string(self.dir.join("voice_styles").join(format!("{id}.json"))).map_err(|_| "Unknown voice.".to_string())?,
            )
            .map_err(e)?;
            let part = |k: &str| -> Result<(Vec<i64>, Vec<f32>), String> {
                let dims: Vec<i64> = serde_json::from_value(v[k]["dims"].clone()).map_err(e)?;
                let mut flat = Vec::new();
                flatten(&v[k]["data"], &mut flat);
                Ok((dims, flat))
            };
            self.styles.insert(id.to_string(), Style { ttl: part("style_ttl")?, dp: part("style_dp")? });
        }
        Ok(&self.styles[id])
    }

    fn ids(&self, text: &str) -> Vec<i64> {
        text.chars()
            .filter_map(|c| self.indexer.get(c as usize).copied())
            .filter(|i| *i >= 0)
            .collect()
    }

    /// One piece of text (up to a few sentences) to samples.
    fn infer(&mut self, text: &str, style_id: &str, speed: f32, seed: u64) -> Result<Vec<f32>, String> {
        let ids = self.ids(text);
        let n = ids.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let (ttl, dp) = {
            let s = self.style(style_id)?;
            (s.ttl.clone(), s.dp.clone())
        };
        let text_ids = || Tensor::from_array(([1usize, n], ids.clone())).map_err(e);
        let text_mask = || Tensor::from_array(([1usize, 1, n], vec![1.0f32; n])).map_err(e);
        let style_ttl = || Tensor::from_array((ttl.0.clone(), ttl.1.clone())).map_err(e);

        let dur = {
            let out = self
                .dp
                .run(ort::inputs! { "text_ids" => text_ids()?, "style_dp" => Tensor::from_array((dp.0.clone(), dp.1.clone())).map_err(e)?, "text_mask" => text_mask()? })
                .map_err(e)?;
            out[0].try_extract_tensor::<f32>().map_err(e)?.1[0] / speed
        };
        let (emb_shape, emb) = {
            let out = self.enc.run(ort::inputs! { "text_ids" => text_ids()?, "style_ttl" => style_ttl()?, "text_mask" => text_mask()? }).map_err(e)?;
            let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(e)?;
            (shape.iter().copied().collect::<Vec<i64>>(), data.to_vec())
        };
        let wav_len = (dur.max(0.1) * self.sample_rate as f32) as usize;
        let latent_len = wav_len.div_ceil(self.chunk);
        let mut rng = Gauss::new(seed);
        let mut xt: Vec<f32> = (0..self.latent_dim * latent_len).map(|_| rng.next()).collect();
        for step in 0..STEPS {
            let out = self
                .est
                .run(ort::inputs! {
                    "noisy_latent" => Tensor::from_array(([1usize, self.latent_dim, latent_len], xt.clone())).map_err(e)?,
                    "text_emb" => Tensor::from_array((emb_shape.clone(), emb.clone())).map_err(e)?,
                    "style_ttl" => style_ttl()?,
                    "text_mask" => text_mask()?,
                    "latent_mask" => Tensor::from_array(([1usize, 1, latent_len], vec![1.0f32; latent_len])).map_err(e)?,
                    "current_step" => Tensor::from_array(([1usize], vec![step as f32])).map_err(e)?,
                    "total_step" => Tensor::from_array(([1usize], vec![STEPS as f32])).map_err(e)?,
                })
                .map_err(e)?;
            xt = out[0].try_extract_tensor::<f32>().map_err(e)?.1.to_vec();
        }
        let out = self.voc.run(ort::inputs! { "latent" => Tensor::from_array(([1usize, self.latent_dim, latent_len], xt)).map_err(e)? }).map_err(e)?;
        let mut wav = out[0].try_extract_tensor::<f32>().map_err(e)?.1.to_vec();
        wav.truncate(wav_len);
        Ok(wav)
    }
}

fn flatten(v: &Value, out: &mut Vec<f32>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| flatten(x, out)),
        Value::Number(n) => out.push(n.as_f64().unwrap_or(0.0) as f32),
        _ => {}
    }
}

/// Standard normal noise (Box–Muller over xorshift), seeded per call.
struct Gauss {
    s: u64,
    spare: Option<f32>,
}

impl Gauss {
    fn new(seed: u64) -> Gauss {
        Gauss { s: seed | 1, spare: None }
    }

    fn uniform(&mut self) -> f32 {
        self.s ^= self.s << 13;
        self.s ^= self.s >> 7;
        self.s ^= self.s << 17;
        ((self.s >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn next(&mut self) -> f32 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let (u, v) = (self.uniform(), self.uniform());
        let r = (-2.0 * u.ln()).sqrt();
        let a = std::f32::consts::TAU * v;
        self.spare = Some(r * a.sin());
        r * a.cos()
    }
}

/// Speaks `text` with a natural voice. Blocking; call from a blocking thread.
pub fn synthesize(text: &str, style: &str, language: Option<&str>, rate: f64) -> Result<Speech, String> {
    let inst = installed().ok_or("Natural voices aren't installed.")?;
    let lang = language.filter(|l| inst.pack.languages.iter().any(|x| x == l)).unwrap_or("en");
    let mut slot = engine_slot().lock().unwrap();
    if slot.is_none() {
        *slot = Some(Engine::load(&inst)?);
    }
    let eng = slot.as_mut().unwrap();
    eng.last_used = Instant::now();
    let max = if matches!(lang, "ko" | "ja") { 120 } else { 300 };
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64);
    let mut samples: Vec<f32> = Vec::new();
    for (k, piece) in chunks(text, max).iter().enumerate() {
        if k > 0 {
            samples.extend(std::iter::repeat(0.0).take(eng.sample_rate as usize * 3 / 10));
        }
        // The model's own "natural" pace is a touch quick at 1.0.
        samples.extend(eng.infer(&prepare(piece, lang), style, 1.05 * rate as f32, seed.wrapping_add(k as u64))?);
    }
    Ok(Speech { rate: eng.sample_rate, samples: samples.iter().map(|s| crate::audio::to_i16(*s)).collect() })
}

/// Loads the voice ahead of the first sentence (voice mode calls this).
pub fn warm() {
    if let Some(inst) = installed() {
        let mut slot = engine_slot().lock().unwrap();
        if slot.is_none() {
            if let Ok(e) = Engine::load(&inst) {
                *slot = Some(e);
            }
        }
    }
}

pub fn unload_if_idle() {
    let mut slot = engine_slot().lock().unwrap();
    if slot.as_ref().is_some_and(|e| e.last_used.elapsed() > IDLE_UNLOAD) {
        *slot = None;
    }
}

// ---------- install ----------

async fn install(state: &Arc<AppState>, cancel: &AtomicBool, emit: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<(), String> {
    let c = &state.catalog.voices;
    let pack = c.models.first().ok_or("No voice pack in the catalog")?;
    emit("engine", 0, 0);
    ensure_runtime(state, cancel, emit).await?;

    let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
    let dir = state.paths.models.join(&pack.id);
    let total = pack.size();
    state.log("network", &format!("Downloading {} ({}) from {}", pack.name, crate::size_label(total), crate::host_of(&pack.files[0].url)));
    let mut done = 0u64;
    for f in &pack.files {
        let dest = dir.join(&f.path);
        let base = done;
        download::fetch_verified(&client, &f.url, &dest, f.size, &f.sha256, cancel, |r, _| emit("download", base + r, total)).await?;
        done += f.size;
    }
    refresh(&state.paths, c);
    // Prove it speaks.
    emit("benchmark", 0, 0);
    tokio::task::spawn_blocking(|| synthesize("Hello.", "F1", Some("en"), 1.0)).await.map_err(|e| e.to_string())??;
    let conn = state.db.lock().unwrap();
    db::log_action(&conn, "model", &format!("Installed {}", pack.name));
    Ok(())
}

#[cfg(test)]
pub async fn install_for_test(state: &Arc<AppState>) -> Result<(), String> {
    install(state, &AtomicBool::new(false), &|_, _, _| {}).await
}

#[derive(Serialize)]
pub struct VoicePackView {
    #[serde(flatten)]
    pack: VoicePack,
    size: u64,
    installed: bool,
}

#[tauri::command(async)]
pub fn voice_packs(state: AppStateRef) -> Vec<VoicePackView> {
    let inst = installed();
    state
        .catalog
        .voices
        .models
        .iter()
        .map(|p| VoicePackView { size: p.size(), installed: inst.as_ref().is_some_and(|i| i.pack.id == p.id), pack: p.clone() })
        .collect()
}

#[tauri::command(async)]
pub fn install_voice_pack(app: AppHandle, state: AppStateRef, pack_id: String) -> Result<(), String> {
    let pack = state.catalog.voices.models.iter().find(|p| p.id == pack_id).ok_or("Unknown voice pack")?.clone();
    if pack.size() > state.budget().disk {
        return Err("There isn't enough free disk space for the voices.".into());
    }
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = crate::claim(&state.installs, &pack_id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
            app.emit("install:progress", json!({ "model_id": pack_id, "phase": phase, "received": received, "total": total })).ok();
        };
        // A cancel deletes only what this install downloaded.
        let dir = state.paths.models.join(&pack.id);
        let files: Vec<PathBuf> = pack.files.iter().map(|f| dir.join(&f.path)).collect();
        let had = download::existing(&files);
        let result = install(&state, &cancel, &emit).await;
        let payload = match &result {
            Ok(()) => json!({ "model_id": pack_id, "name": pack.name, "ok": true }),
            Err(e) if e == download::CANCELLED => {
                download::discard_new(&files, &had, &state.paths.models);
                json!({ "model_id": pack_id, "name": pack.name, "ok": false, "cancelled": true })
            }
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", pack.name));
                json!({ "model_id": pack_id, "name": pack.name, "ok": false, "error": e })
            }
        };
        app.emit("install:finished", payload).ok();
    });
    Ok(())
}

#[tauri::command(async)]
pub fn remove_voice_pack(state: AppStateRef, pack_id: String) -> Result<(), String> {
    let pack = state.catalog.voices.models.iter().find(|p| p.id == pack_id).ok_or("Unknown voice pack")?;
    *engine_slot().lock().unwrap() = None;
    let dir = state.paths.models.join(&pack.id);
    if dir.starts_with(&state.paths.models) && dir != state.paths.models && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the voices: {e}"))?;
    }
    refresh(&state.paths, &state.catalog.voices);
    let conn = state.db.lock().unwrap();
    crate::speech::update_voice_settings(&conn, |s| {
        if s.voice.as_deref().is_some_and(|v| v.starts_with("supertonic:")) {
            s.voice = None;
        }
    })?;
    db::log_action(&conn, "model", &format!("Removed {} and deleted its files", pack.name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_prepared_like_the_training_data() {
        assert_eq!(prepare("Hello \u{2014} world", "en"), "<en>Hello - world.</en>");
        assert_eq!(prepare("Café “ok” 😀", "fr"), "<fr>Cafe\u{301} \"ok\"</fr>", "a closing quote ends it too");
        assert_eq!(prepare("Is it done ?", "en"), "<en>Is it done?</en>");
        assert_eq!(prepare("mail me @ home", "en"), "<en>mail me at home.</en>");
    }

    #[test]
    fn long_text_is_chunked_at_sentences() {
        let text = "One two three. Four five six. Seven eight nine.";
        assert_eq!(chunks(text, 20), vec!["One two three.", "Four five six.", "Seven eight nine."]);
        assert_eq!(chunks(text, 300), vec![text]);
        assert_eq!(chunks("First.\n\nSecond.", 300), vec!["First.", "Second."]);
    }

    #[test]
    fn noise_is_roughly_standard_normal() {
        let mut g = Gauss::new(42);
        let xs: Vec<f32> = (0..20_000).map(|_| g.next()).collect();
        let mean = xs.iter().sum::<f32>() / xs.len() as f32;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32;
        assert!(mean.abs() < 0.03, "{mean}");
        assert!((var - 1.0).abs() < 0.05, "{var}");
    }

    #[test]
    fn the_catalog_pack_is_complete() {
        let c = crate::catalog::Catalog::bundled().voices;
        let p = &c.models[0];
        for name in ["duration_predictor", "text_encoder", "vector_estimator", "vocoder"] {
            assert!(p.files.iter().any(|f| f.path == format!("onnx/{name}.onnx")), "{name}");
        }
        assert_eq!(p.styles.len(), 10);
        assert!(p.languages.contains(&"de".to_string()));
        assert!(p.files.iter().all(|f| f.sha256.len() == 64 && f.url.contains(&f.path)));
    }
}

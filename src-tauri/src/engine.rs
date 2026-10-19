// SPDX-License-Identifier: AGPL-3.0-only
//! Installs llama.cpp and runs `llama-server` as a private, local process.
//!
//! The server listens only on 127.0.0.1, on a random port, and requires a
//! random API key, so other programs on the PC can't use it uninvited.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::process::{Child, Command};

use crate::catalog::{Asset, Budget, EngineSpec};
use crate::download;
use crate::net::{self, Connectivity, Purpose};
use crate::paths::Paths;

const SERVER_EXE: &str = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };

/// Picks the engine build for this PC. Vulkan runs on NVIDIA, AMD and Intel
/// graphics cards alike (Windows and Linux); Macs use Metal, which the macOS
/// builds include. A CUDA build for NVIDIA is a later speed-up.
pub fn backend_for(budget: &Budget) -> Result<&'static str, String> {
    backend_named(std::env::consts::OS, std::env::consts::ARCH, budget.vram > 0)
}

/// The key for processor-only downloads on this system ("cpu-x64" on
/// Windows, as the catalog has always named it).
pub fn cpu_key() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x64",
        _ => "cpu-x64",
    }
}

/// A program on the system's PATH (macOS and Linux tools the user installed).
pub fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

fn backend_named(os: &str, arch: &str, gpu: bool) -> Result<&'static str, String> {
    Ok(match (os, arch, gpu) {
        ("windows", "x86_64", true) => "vulkan-x64",
        ("windows", "x86_64", false) => "cpu-x64",
        ("linux", "x86_64", true) => "linux-vulkan-x64",
        ("linux", "x86_64", false) => "linux-cpu-x64",
        ("linux", "aarch64", true) => "linux-vulkan-arm64",
        ("linux", "aarch64", false) => "linux-cpu-arm64",
        ("macos", "aarch64", _) => "macos-arm64",
        ("macos", "x86_64", _) => "macos-x64",
        _ => return Err(format!("Running models isn't supported on {os} ({arch}) yet.")),
    })
}

/// Past this, a running server's log starts over (a model chatting all day
/// mustn't fill the disk with its log).
const LOG_CAP: u64 = 64 * 1024 * 1024;

/// A fresh log for a server: its output and errors, appended (so it can be
/// cut while the server runs), and kept under `LOG_CAP`.
pub fn capped_log(path: &Path) -> std::io::Result<(std::fs::File, std::fs::File)> {
    std::fs::File::create(path)?;
    let out = std::fs::OpenOptions::new().append(true).open(path)?;
    let err = out.try_clone()?;
    static WATCHED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);
    let first = {
        let mut w = WATCHED.lock().unwrap();
        let first = w.is_empty();
        w.insert(path.to_path_buf());
        first
    };
    if first {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            for p in WATCHED.lock().unwrap().iter() {
                if std::fs::metadata(p).is_ok_and(|m| m.len() > LOG_CAP) {
                    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(p) {
                        let _ = f.set_len(0);
                    }
                }
            }
        });
    }
    Ok((out, err))
}

/// Linux: the kernel stops the program if the app dies, even in a crash
/// (Windows uses a job object; on macOS a normal quit stops it).
pub fn tie_to_app(cmd: &mut Command) {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe and touches only the child.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

/// The graphics devices an engine sees, as `--list-devices` prints them:
/// ("Vulkan0", "NVIDIA GeForce RTX 4070"), in the engine's own order.
static DEVICES: std::sync::Mutex<Option<(PathBuf, Vec<(String, String)>)>> = std::sync::Mutex::new(None);

fn parse_devices(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (id, rest) = l.trim().split_once(": ")?;
            let id_ok = id.chars().last().is_some_and(|c| c.is_ascii_digit()) && id.chars().all(|c| c.is_ascii_alphanumeric());
            let name = rest.rsplit_once(" (").map_or(rest, |(n, _)| n).trim();
            (id_ok && !name.is_empty()).then(|| (id.to_string(), name.to_string()))
        })
        .collect()
}

/// Of the devices an engine sees, the dedicated graphics cards, when the PC
/// also has integrated graphics the engine could otherwise use. Matched by
/// name against the cards Windows reports.
fn dedicated_of(devices: &[(String, String)], cards: &[crate::hardware::Gpu]) -> Vec<String> {
    let same = |a: &str, b: &str| {
        let (a, b) = (a.to_lowercase(), b.to_lowercase());
        a == b || a.contains(&b) || b.contains(&a)
    };
    let is_integrated = |name: &str| cards.iter().any(|c| c.integrated && same(name, &c.name));
    let is_dedicated = |name: &str| cards.iter().any(|c| !c.integrated && same(name, &c.name));
    let dedicated: Vec<String> = devices.iter().filter(|(_, n)| is_dedicated(n) && !is_integrated(n)).map(|(id, _)| id.clone()).collect();
    if dedicated.is_empty() || dedicated.len() == devices.len() {
        return Vec::new();
    }
    dedicated
}

/// The dedicated cards for `exe` (a llama.cpp program), asked once per engine.
async fn dedicated_devices(exe: &Path) -> Vec<String> {
    let known = DEVICES.lock().unwrap().as_ref().filter(|(p, _)| p == exe).map(|(_, d)| d.clone());
    let devices = match known {
        Some(d) => d,
        None => {
            let mut cmd = Command::new(exe);
            cmd.arg("--list-devices").stdin(Stdio::null()).kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000);
            let out = match tokio::time::timeout(Duration::from_secs(20), cmd.output()).await {
                Ok(Ok(o)) => format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
                _ => String::new(),
            };
            let d = parse_devices(&out);
            *DEVICES.lock().unwrap() = Some((exe.to_path_buf(), d.clone()));
            d
        }
    };
    dedicated_of(&devices, &crate::hardware::cards())
}

/// Keeps a graphics engine (pictures, speech, training) on the dedicated
/// card, the way the chat engine is: Vulkan engines otherwise may also pick
/// the integrated one. Uses what the chat engine found; its numbers match
/// the engine's only when it saw every card, so nothing is set otherwise.
pub fn prefer_dedicated(cmd: &mut Command) {
    if let Some(picked) = dedicated_indexes() {
        cmd.env("GGML_VK_VISIBLE_DEVICES", picked);
    }
}

/// The dedicated cards' Vulkan numbers ("0"), when known for sure: the
/// engine listed every card, and some are integrated.
fn dedicated_indexes() -> Option<String> {
    let (_, devices) = DEVICES.lock().unwrap().clone()?;
    let cards = crate::hardware::cards();
    if devices.len() != cards.len() || std::env::var_os("GGML_VK_VISIBLE_DEVICES").is_some() {
        return None;
    }
    let picked: Vec<String> = dedicated_of(&devices, &cards).iter().filter_map(|id| id.strip_prefix("Vulkan").map(str::to_string)).collect();
    (!picked.is_empty()).then(|| picked.join(","))
}

/// Before a graphics engine runs (pictures, speech): learns which card is
/// which from the installed chat engine, if no chat model did yet this run,
/// so the engine can be kept off the integrated graphics. True when the
/// dedicated card is known and engines will be pinned to it.
pub async fn learn_devices(state: &crate::AppState) -> bool {
    let known = DEVICES.lock().unwrap().is_some();
    if !known {
        let exe = backend_for(&state.budget()).ok().and_then(|b| installed_server(&state.paths, &state.catalog.engine, b));
        if let Some(exe) = exe {
            dedicated_devices(&exe).await;
            if let Some((_, d)) = DEVICES.lock().unwrap().as_ref() {
                crate::applog::line("engine", &format!("Graphics devices the engines see: {}", d.iter().map(|(id, name)| format!("{id} {name}")).collect::<Vec<_>>().join(", ")));
            }
        }
    }
    dedicated_indexes().is_some()
}

/// Finds a program by file name anywhere under `dir`.
pub fn find_exe(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_exe(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

pub fn installed_server(paths: &Paths, spec: &EngineSpec, backend: &str) -> Option<PathBuf> {
    find_exe(&paths.engines.join(format!("{}-{backend}", spec.build)), SERVER_EXE)
}

/// Downloads, verifies and unpacks the engine if it isn't installed yet.
pub async fn ensure_installed(
    paths: &Paths,
    spec: &EngineSpec,
    backend: &str,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let asset = spec
        .assets
        .get(backend)
        .ok_or_else(|| format!("No engine build for {backend}"))?;
    let dir = format!("{}-{backend}", spec.build);
    ensure_unpacked(paths, &dir, asset, SERVER_EXE, cancel, progress).await
}

/// Makes sure `engines/<dir>` holds the unpacked release zip and returns the
/// path of `exe` inside it.
pub async fn ensure_unpacked(
    paths: &Paths,
    dir_name: &str,
    asset: &Asset,
    exe: &str,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let dir = paths.engines.join(dir_name);
    if let Some(found) = find_exe(&dir, exe) {
        return Ok(found);
    }
    let tarball = asset.url.ends_with(".tar.gz") || asset.url.ends_with(".tgz");
    let zip_path = paths.engines.join(format!("{dir_name}.{}", if tarball { "tar.gz" } else { "zip" }));
    // Engine downloads are user-started installs, allowed at every level.
    let client = net::external_client(Connectivity::Offline, Purpose::ModelDownload, false)?;
    download::fetch_verified(&client, &asset.url, &zip_path, asset.size, &asset.sha256, cancel, progress).await?;

    let staging = paths.engines.join(format!(".staging-{dir_name}"));
    let (zip_c, staging_c) = (zip_path.clone(), staging.clone());
    tokio::task::spawn_blocking(move || if tarball { untar(&zip_c, &staging_c) } else { unzip(&zip_c, &staging_c) })
        .await
        .map_err(|e| e.to_string())??;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&staging, &dir).map_err(|e| e.to_string())?;
    std::fs::remove_file(&zip_path).ok();
    find_exe(&dir, exe).ok_or_else(|| format!("The engine download didn't contain {exe}."))
}

/// Unpacks a .tar.gz (Linux and macOS builds), keeping their links and
/// permissions; entries reaching outside `into` are refused by `tar`.
fn untar(path: &Path, into: &Path) -> Result<(), String> {
    if into.exists() {
        std::fs::remove_dir_all(into).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
    archive.set_preserve_permissions(true);
    archive.unpack(into).map_err(|e| format!("Couldn't unpack the engine: {e}"))
}

fn unzip(zip_path: &Path, into: &Path) -> Result<(), String> {
    if into.exists() {
        std::fs::remove_dir_all(into).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name rejects absolute paths and `..` escapes.
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = into.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut dest = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut dest).map_err(|e| e.to_string())?;
        // Zips made on Linux and macOS keep programs' execute permission.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub struct LaunchSpec<'a> {
    pub exe: &'a Path,
    pub model_path: &'a Path,
    pub model_id: &'a str,
    pub quant: &'a str,
    pub ctx: u32,
    pub gpu_layers: u32,
    /// Processor threads (0 = the engine's own choice).
    pub threads: usize,
    /// Below-normal priority, so the rest of the PC stays responsive.
    pub low_priority: bool,
    pub log: &'a Path,
    /// The image encoder, for models that can see pictures.
    pub mmproj: Option<&'a Path>,
    /// Prompt batch size (0 = the engine's default).
    pub batch: u32,
    /// A taught adapter (LoRA) loaded on top of the model.
    pub lora: Option<&'a Path>,
}

/// Launch settings beyond `LaunchSpec`, from the fit plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tuning {
    /// A mixture-of-experts model split with the graphics card: every layer
    /// goes on the card, but the experts of the first N layers stay in
    /// system memory (--n-cpu-moe). `gpu_layers` then says how much of the
    /// model that is, in whole layers.
    pub cpu_moe: Option<u32>,
    /// K and V kept as q8_0 (`catalog::with_q8_cache`): about half the
    /// memory, so the whole model or a bigger window fits the card.
    pub kv_q8: bool,
}

struct Running {
    child: Child,
    port: u16,
    key: String,
    model_id: String,
    /// What it was asked for: the model, or a taught model's own id.
    asked: String,
    quant: String,
    ctx: u32,
    gpu_layers: u32,
    threads: usize,
    low_priority: bool,
    vision: bool,
    batch: u32,
    lora: Option<PathBuf>,
    cpu_moe: Option<u32>,
    kv_q8: bool,
}

impl Running {
    fn endpoint(&self) -> Endpoint {
        Endpoint { port: self.port, key: self.key.clone(), ctx: self.ctx, extra: serde_json::Value::Null, cloud: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub model_id: Option<String>,
    pub quant: Option<String>,
    pub ctx: Option<u32>,
    pub gpu_layers: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub port: u16,
    pub key: String,
    pub ctx: u32,
    /// Fields merged into each chat request (advanced sampling settings).
    pub extra: serde_json::Value,
    /// Set when the model runs at a cloud provider instead of on this PC.
    pub cloud: Option<std::sync::Arc<crate::cloud::Target>>,
}

impl Endpoint {
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

#[derive(Default)]
pub struct Engine {
    running: Option<Running>,
    /// A model paused to give a picture the graphics card: its address, so
    /// it comes back where the chat that asked for the picture expects it.
    paused: Option<Paused>,
}

/// What a paused model needs to come back as it was.
#[derive(Debug, Clone)]
pub struct Paused {
    /// What to ask for to bring it back: the model, or a taught model's own
    /// id, so its adapter comes back with it.
    pub model_id: String,
    pub vision: bool,
    base: String,
    lora: Option<PathBuf>,
    port: u16,
    key: String,
}

impl Engine {
    fn alive(&mut self) -> bool {
        match self.running.as_mut() {
            Some(r) => matches!(r.child.try_wait(), Ok(None)),
            None => false,
        }
    }

    pub fn status(&mut self) -> EngineStatus {
        if !self.alive() {
            self.running = None;
        }
        let r = self.running.as_ref();
        EngineStatus {
            model_id: r.map(|r| r.model_id.clone()),
            quant: r.map(|r| r.quant.clone()),
            ctx: r.map(|r| r.ctx),
            gpu_layers: r.map(|r| r.gpu_layers),
        }
    }

    /// The model running now, as launched: (process id, model, quantization,
    /// context, GPU layers). Loading another one frees what it holds.
    pub fn loaded(&mut self) -> Option<(Option<u32>, String, String, u32, u32)> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        Some((r.child.id(), r.model_id.clone(), r.quant.clone(), r.ctx, r.gpu_layers))
    }

    /// The running endpoint, if it is already serving this model with this
    /// taught adapter (or none): the base model doesn't answer for a taught
    /// one, nor the other way round.
    pub fn endpoint_for(&mut self, model_id: &str, lora: Option<&Path>) -> Option<Endpoint> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        (r.model_id == model_id && r.lora.as_deref() == lora).then(|| r.endpoint())
    }

    /// The model running now, whatever it is: its id and endpoint.
    pub fn current(&mut self) -> Option<(String, Endpoint)> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        Some((r.model_id.clone(), r.endpoint()))
    }

    /// Notes what the running model was asked for (a taught model's own id),
    /// so a pause brings back the same thing.
    pub fn asked_as(&mut self, id: &str) {
        if let Some(r) = self.running.as_mut() {
            r.asked = id.to_string();
        }
    }

    /// Whether the running server already matches this launch exactly.
    #[cfg(test)]
    pub fn matches(&mut self, spec: &LaunchSpec<'_>) -> bool {
        self.matches_tuned(spec, Tuning::default())
    }

    pub fn matches_tuned(&mut self, spec: &LaunchSpec<'_>, tuning: Tuning) -> bool {
        self.alive()
            && self.running.as_ref().is_some_and(|r| {
                r.model_id == spec.model_id
                    && r.quant == spec.quant
                    && r.ctx == spec.ctx
                    && r.gpu_layers == spec.gpu_layers
                    && r.threads == spec.threads
                    && r.low_priority == spec.low_priority
                    && r.vision == spec.mmproj.is_some()
                    && r.batch == spec.batch
                    && r.lora.as_deref() == spec.lora
                    && r.cpu_moe == tuning.cpu_moe
                    && r.kv_q8 == tuning.kv_q8
            })
    }

    /// The running model was started with its image encoder.
    pub fn sees(&mut self) -> bool {
        self.alive() && self.running.as_ref().is_some_and(|r| r.vision)
    }

    /// Starts the server for this model, replacing any other running model
    /// (or the same one launched with other limits).
    pub async fn ensure(&mut self, spec: LaunchSpec<'_>, job: Option<&JobRef>) -> Result<Endpoint, String> {
        self.ensure_tuned(spec, Tuning::default(), job).await
    }

    pub async fn ensure_tuned(&mut self, spec: LaunchSpec<'_>, tuning: Tuning, job: Option<&JobRef>) -> Result<Endpoint, String> {
        if self.matches_tuned(&spec, tuning) {
            return Ok(self.running.as_ref().unwrap().endpoint());
        }
        // The same model with other limits (mode, heat, its picture reader)
        // keeps its address and key, so a reply holding them carries on.
        let same = |model: &str, lora: Option<&Path>| model == spec.model_id && lora == spec.lora;
        let relaunch = self.running.as_ref().filter(|r| same(&r.model_id, r.lora.as_deref())).map(|r| (r.port, r.key.clone()));
        self.stop().await;

        // Coming back from a pause: the same address too, if it's still free.
        let back = self.paused.take().filter(|p| same(&p.base, p.lora.as_deref())).map(|p| (p.port, p.key));
        let (port, key) = match relaunch.or(back).filter(|(port, _)| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok()) {
            Some(p) => p,
            None => (free_port()?, uuid::Uuid::new_v4().simple().to_string()),
        };
        let (log, log_err) = capped_log(spec.log).map_err(|e| e.to_string())?;

        // On a PC with integrated graphics too, the model goes on the dedicated card.
        let devices = if spec.gpu_layers > 0 { dedicated_devices(spec.exe).await } else { Vec::new() };
        let mut cmd = Command::new(spec.exe);
        cmd.args(if devices.is_empty() { vec![] } else { vec!["--device".to_string(), devices.join(",")] });
        // Experts kept in system memory: every layer on the card otherwise.
        let layers = match tuning.cpu_moe {
            Some(n) => vec!["-ngl".to_string(), "999".to_string(), "--n-cpu-moe".to_string(), n.to_string()],
            None => vec!["-ngl".to_string(), spec.gpu_layers.to_string()],
        };
        cmd.arg("-m").arg(spec.model_path)
            .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--api-key", &key])
            .args(["-c", &spec.ctx.to_string()])
            .args(layers)
            // One conversation at a time gets the whole context window.
            .args(["-np", "1", "--jinja", "--no-webui"])
            .args(if spec.threads > 0 { vec!["-t".to_string(), spec.threads.to_string()] } else { vec![] })
            .args(if spec.batch > 0 { vec!["-b".to_string(), spec.batch.to_string()] } else { vec![] })
            .args(if tuning.kv_q8 { vec!["-ctk", "q8_0", "-ctv", "q8_0"] } else { vec![] })
            .args(spec.lora.map(|p| vec![std::ffi::OsString::from("--lora"), p.into()]).unwrap_or_default())
            .args(spec.mmproj.map(|p| vec![std::ffi::OsString::from("--mmproj"), p.into()]).unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            cmd.creation_flags(CREATE_NO_WINDOW | if spec.low_priority { BELOW_NORMAL_PRIORITY_CLASS } else { 0 });
        }
        crate::engine::tie_to_app(&mut cmd);
        let child = cmd.spawn().map_err(|e| format!("Couldn't start the engine: {e}"))?;
        #[cfg(windows)]
        if let (Some(job), Some(pid)) = (job, child.id()) {
            job.assign(pid).ok();
        }
        #[cfg(not(windows))]
        let _ = job;

        self.running = Some(Running {
            child,
            port,
            key: key.clone(),
            model_id: spec.model_id.to_string(),
            asked: spec.model_id.to_string(),
            quant: spec.quant.to_string(),
            ctx: spec.ctx,
            gpu_layers: spec.gpu_layers,
            threads: spec.threads,
            low_priority: spec.low_priority,
            vision: spec.mmproj.is_some(),
            batch: spec.batch,
            lora: spec.lora.map(Path::to_path_buf),
            cpu_moe: tuning.cpu_moe,
            kv_q8: tuning.kv_q8,
        });

        let endpoint = Endpoint { port, key, ctx: spec.ctx, extra: serde_json::Value::Null, cloud: None };
        let started = Instant::now();
        let what = format!(
            "{} ({}, {} context, {}{}{}{})",
            spec.model_id,
            spec.quant,
            spec.ctx,
            match tuning.cpu_moe {
                Some(n) => format!("every layer on the graphics card, the experts of {n} in memory"),
                None => format!("{} layers on the graphics card", spec.gpu_layers),
            },
            if tuning.kv_q8 { ", compressed cache" } else { "" },
            if devices.is_empty() { String::new() } else { format!(", device {}", devices.join(",")) },
            if spec.mmproj.is_some() { ", sees pictures" } else { "" }
        );
        if let Err(e) = self.wait_ready(&endpoint, spec.log).await {
            crate::applog::line("engine", &format!("Couldn't load {what}: {e}"));
            self.stop().await;
            return Err(e);
        }
        crate::applog::line("engine", &format!("Loaded {what} in {:.1} s", started.elapsed().as_secs_f64()));
        Ok(endpoint)
    }

    async fn wait_ready(&mut self, ep: &Endpoint, log: &Path) -> Result<(), String> {
        let client = net::local_client();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(300) {
            if !self.alive() {
                return Err(format!("The engine stopped while loading the model.\n{}", log_tail(log, 12)));
            }
            if let Ok(resp) = client.get(ep.url("/health")).timeout(Duration::from_secs(2)).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        Err("The model took too long to load.".into())
    }

    pub async fn stop(&mut self) {
        if let Some(mut r) = self.running.take() {
            r.child.start_kill().ok();
            let _ = tokio::time::timeout(Duration::from_secs(5), r.child.wait()).await;
        }
    }

    /// Stops the model for a while (a picture needs the graphics card), keeping
    /// its address for when it's started again. What was paused, if anything.
    pub async fn pause(&mut self) -> Option<Paused> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        let p = Paused { model_id: r.asked.clone(), vision: r.vision, base: r.model_id.clone(), lora: r.lora.clone(), port: r.port, key: r.key.clone() };
        self.stop().await;
        self.paused = Some(p.clone());
        Some(p)
    }
}

/// Which replies were given which running model, so a request for another
/// model doesn't stop it under them. A reply is known by its stop flag (the
/// app's `generations`), a model by its key ("" for a cloud model).
#[derive(Default)]
pub struct Users(Vec<User>);

struct User {
    flag: Arc<AtomicBool>,
    key: String,
    /// The task that asked, whose own later requests don't wait on it.
    task: Option<tokio::task::Id>,
    /// It was the only reply waiting for a model when it was given this one;
    /// otherwise any of them may have been the one asking.
    sure: bool,
}

pub static USERS: std::sync::Mutex<Users> = std::sync::Mutex::new(Users(Vec::new()));

impl Users {
    fn has(&self, flag: &Arc<AtomicBool>) -> bool {
        self.0.iter().any(|u| Arc::ptr_eq(&u.flag, flag))
    }

    /// Of the replies running now, the ones not given a model yet.
    pub fn unplaced(&self, running: &[Arc<AtomicBool>]) -> Vec<Arc<AtomicBool>> {
        running.iter().filter(|f| !self.has(f)).cloned().collect()
    }

    /// The model with `key` was given to `task`, asking for one of `before`
    /// (the replies waiting for a model when it asked, as `unplaced` found
    /// them), or for none. Replies that ended are forgotten.
    pub fn place(&mut self, running: &[Arc<AtomicBool>], before: &[Arc<AtomicBool>], key: &str, task: Option<tokio::task::Id>) {
        let alive = |f: &Arc<AtomicBool>| running.iter().any(|r| Arc::ptr_eq(r, f));
        self.0.retain(|u| alive(&u.flag));
        let fresh: Vec<&Arc<AtomicBool>> = before.iter().filter(|f| alive(f) && !self.has(f)).collect();
        let sure = fresh.len() == 1;
        for f in fresh {
            self.0.push(User { flag: f.clone(), key: key.to_string(), task, sure });
        }
    }

    /// Replies other than the asking one still using the model with `key`
    /// (ones being stopped don't count). `task` and `before` are the asking
    /// request's, as for `place`. A reply that may be the asking one's own
    /// is let off, once: given the model along with others, or not given one
    /// yet (a scheduled task or a reply to another chat runs on the model
    /// loaded already).
    pub fn others(&self, running: &[Arc<AtomicBool>], before: &[Arc<AtomicBool>], key: &str, task: Option<tokio::task::Id>) -> usize {
        let live = |f: &Arc<AtomicBool>| !f.load(Ordering::Relaxed) && running.iter().any(|r| Arc::ptr_eq(r, f));
        let mine = |u: &User| task.is_some() && u.task == task;
        let on_it: Vec<&User> = self.0.iter().filter(|u| u.key == key && live(&u.flag) && !mine(u)).collect();
        let sure = on_it.iter().filter(|u| u.sure).count();
        let maybe = on_it.len() - sure + before.iter().filter(|f| live(f) && !self.has(f)).count();
        sure + maybe.saturating_sub(1)
    }
}

#[cfg(windows)]
pub type JobRef = crate::winjob::Job;
#[cfg(not(windows))]
pub type JobRef = ();

pub fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

pub fn log_tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Measures generation speed (tokens/second) on this PC.
/// A model's speed on this PC: writing (tokens a second it produces) and
/// reading (tokens a second it takes in, for long prompts and documents).
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct Speed {
    pub tps: f64,
    pub prompt_tps: Option<f64>,
}

/// About 600 tokens to read, so the reading speed means something.
const READING: &str = "Notes from the planning meeting. The team reviewed the budget for the next quarter, the hiring plan, \
    and the schedule for the product launch. Marketing asked for two more weeks to finish the campaign, and engineering \
    agreed to move the release by one week to fix the remaining issues in the sync feature. Finance confirmed that travel \
    costs were under budget and suggested moving the savings to the support team, which has been handling more requests \
    since the last update. The design group showed new screens for the mail and calendar pages and asked for feedback by \
    Friday. Everyone agreed to meet again next Tuesday at ten to confirm the launch date and the final budget numbers.";

/// Writing and reading speed, with the same settings chats use. The prompt
/// cache is off, so a second test reads the prompt again rather than reusing it.
pub async fn measure(ep: &Endpoint) -> Result<Speed, String> {
    // A short warm-up first: the graphics card's first answer after loading
    // runs well below its usual speed (49 vs 72 tokens/sec on Qwen3-VL 4B).
    let warm = serde_json::json!({ "messages": [{ "role": "user", "content": "Hi" }], "max_tokens": 16, "cache_prompt": false });
    let _ = net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&warm).timeout(Duration::from_secs(120)).send().await;
    let prompt = format!("{}\n\n{}\n\n{}\n\nWrite three short sentences about the ocean.", READING, READING, READING);
    let body = serde_json::json!({
        "messages": [{ "role": "user", "content": prompt }],
        "max_tokens": 128,
        "temperature": 0.7,
        "cache_prompt": false,
    });
    let started = Instant::now();
    let resp: serde_json::Value = net::local_client()
        .post(ep.url("/v1/chat/completions"))
        .bearer_auth(&ep.key)
        .json(&body)
        .timeout(Duration::from_secs(180))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let round = |v: f64| (v * 10.0).round() / 10.0;
    let prompt_tps = resp.pointer("/timings/prompt_per_second").and_then(|v| v.as_f64()).filter(|v| v.is_finite() && *v > 0.0).map(round);
    if let Some(tps) = resp.pointer("/timings/predicted_per_second").and_then(|v| v.as_f64()) {
        return Ok(Speed { tps: round(tps), prompt_tps });
    }
    let tokens = resp.pointer("/usage/completion_tokens").and_then(|v| v.as_f64()).unwrap_or(0.0);
    Ok(Speed { tps: round(tokens / started.elapsed().as_secs_f64()), prompt_tps })
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    #[test]
    fn the_model_goes_on_the_dedicated_card() {
        use crate::hardware::Gpu;
        let listed = parse_devices("Available devices:
  Vulkan0: NVIDIA GeForce RTX 5070 Ti Laptop GPU (11944 MiB, 11176 MiB free)
  Vulkan1: Intel(R) Graphics (18347 MiB, 17579 MiB free)
");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1], ("Vulkan1".to_string(), "Intel(R) Graphics".to_string()));
        let card = |name: &str, integrated: bool| Gpu { name: name.into(), integrated, ..Default::default() };
        let laptop = [card("NVIDIA GeForce RTX 5070 Ti Laptop GPU", false), card("Intel(R) Graphics", true)];
        assert_eq!(dedicated_of(&listed, &laptop), vec!["Vulkan0"]);
        // Only a dedicated card, or only integrated graphics: nothing to choose.
        assert!(dedicated_of(&listed[..1], &laptop[..1]).is_empty());
        assert!(dedicated_of(&listed[1..], &laptop[1..]).is_empty());
        // Names that don't match what Windows reports: the engine decides.
        assert!(dedicated_of(&listed, &[card("Some other card", false)]).is_empty());
    }

    /// On any system: downloads this system's engine build into
    /// SULCUSAI_DATA (a scratch folder), loads SULCUSAI_MODEL (a Qwen3 GGUF)
    /// and gets a streamed answer.
    #[tokio::test]
    #[ignore]
    async fn e2e_engine_runs_on_this_system() {
        let data = std::path::PathBuf::from(std::env::var("SULCUSAI_DATA").expect("SULCUSAI_DATA"));
        let model = std::path::PathBuf::from(std::env::var("SULCUSAI_MODEL").expect("SULCUSAI_MODEL"));
        let paths = Paths::new(data).unwrap();
        let hw = crate::hardware::detect(&paths.models);
        println!("{} · {} · gpus {:?}", hw.os, hw.cpu_name, hw.gpus);
        let budget = Budget::from_hardware(&hw);
        let backend = backend_for(&budget).unwrap();
        let cat = crate::catalog::Catalog::bundled();
        let cancel = AtomicBool::new(false);
        let exe = ensure_installed(&paths, &cat.engine, backend, &cancel, |_, _| {}).await.unwrap();
        println!("engine {backend}: {}", exe.display());
        let mut eng = Engine::default();
        let log = paths.engine_log();
        let ep = eng
            .ensure(
                LaunchSpec {
                    exe: &exe,
                    model_path: &model,
                    model_id: "qwen3-1.7b",
                    quant: "Q4_K_M",
                    ctx: 4096,
                    gpu_layers: if budget.vram > 0 { 99 } else { 0 },
                    threads: 0,
                    low_priority: false,
                    log: &log,
                    mmproj: None,
                    batch: 0,
                    lora: None,
                },
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("engine failed to start: {e}"));
        let messages = vec![serde_json::json!({ "role": "user", "content": "What is 2 + 2? Answer with just the number. /no_think" })];
        let mut deltas = 0;
        let done = crate::chat::stream(&ep, messages, None, &cancel, |_| deltas += 1).await.unwrap();
        println!("reply ({deltas} deltas): {:?} · tps {:?}", done.content, done.tps);
        assert!(done.content.contains('4'));
        eng.stop().await;
    }

    /// A reply keeps the model it was given: a request for another model
    /// waits for it, the reply's own later requests don't, and a reply that
    /// was stopped or ended doesn't hold anything.
    #[tokio::test]
    async fn a_reply_keeps_the_model_it_was_given() {
        let task = || async { tokio::spawn(async { tokio::task::id() }).await.unwrap() };
        let (ta, tb, tc) = (Some(task().await), Some(task().await), Some(task().await));
        let flag = || Arc::new(AtomicBool::new(false));
        let (a, b) = (flag(), flag());
        let mut users = Users::default();

        // Chat A asks for model X and gets it.
        let before_a = users.unplaced(&[a.clone()]);
        users.place(&[a.clone()], &before_a, "x", ta);
        // Chat B wants another model: A is still on X.
        let running = [a.clone(), b.clone()];
        let before_b = users.unplaced(&running);
        assert_eq!(before_b.len(), 1);
        assert_eq!(users.others(&running, &before_b, "x", tb), 1);
        // A's own later request (a picture, say) doesn't wait on itself.
        assert_eq!(users.others(&running, &[], "x", ta), 0);
        // So does a request that isn't a chat (the Studio, a speed test).
        assert_eq!(users.others(&[a.clone()], &[], "x", tc), 1);
        // Another model's replies, and the cloud's, don't hold X.
        assert_eq!(users.others(&running, &before_b, "y", tb), 0);
        users.place(&running, &before_b, "", tb);
        assert_eq!(users.others(&running, &[], "x", tc), 1, "B, on the cloud, doesn't count");
        // A stopped, or finished: X is free.
        a.store(true, Ordering::Relaxed);
        assert_eq!(users.others(&running, &[], "x", tc), 0);
        a.store(false, Ordering::Relaxed);
        assert_eq!(users.others(&[b.clone()], &[], "x", tc), 0);
    }

    /// Two chats starting at once: either may be the one asking, so the
    /// second waits only for the other one, never for itself.
    #[tokio::test]
    async fn two_chats_at_once_dont_wait_on_themselves() {
        let task = || async { tokio::spawn(async { tokio::task::id() }).await.unwrap() };
        let (ta, tb) = (Some(task().await), Some(task().await));
        let (a, b) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let mut users = Users::default();
        let running = [a.clone(), b.clone()];
        // A's request came in after both chats started: both look like A's.
        let before_a = users.unplaced(&running);
        users.place(&running, &before_a, "x", ta);
        // B wants another model: it waits for A...
        let before_b = users.unplaced(&running);
        assert_eq!(users.others(&running, &before_b, "x", tb), 1);
        // ...and only for A.
        assert_eq!(users.others(&[b.clone()], &before_b, "x", tb), 0);
        // A task that ran before the first chat got a model (a scheduled
        // task, a reply to another chat) still holds it for a chat's request.
        let s = Arc::new(AtomicBool::new(false));
        let fresh = Users::default();
        let running = [s.clone(), b.clone()];
        let before = fresh.unplaced(&running);
        assert_eq!(fresh.others(&running, &before, "x", tb), 1);
        assert_eq!(fresh.others(&[b.clone()], &[b.clone()], "x", tb), 0);
    }

    #[test]
    fn engine_builds_per_system() {
        assert_eq!(backend_named("windows", "x86_64", true).unwrap(), "vulkan-x64");
        assert_eq!(backend_named("linux", "x86_64", false).unwrap(), "linux-cpu-x64");
        assert_eq!(backend_named("macos", "aarch64", true).unwrap(), "macos-arm64");
        assert!(backend_named("freebsd", "x86_64", true).is_err());
    }
}

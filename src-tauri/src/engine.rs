// SPDX-License-Identifier: AGPL-3.0-only
//! Installs llama.cpp and runs `llama-server` as a private, local process.
//!
//! The server listens only on 127.0.0.1, on a random port, and requires a
//! random API key, so other programs on the PC can't use it uninvited.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
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

struct Running {
    child: Child,
    port: u16,
    key: String,
    model_id: String,
    quant: String,
    ctx: u32,
    gpu_layers: u32,
    threads: usize,
    low_priority: bool,
    vision: bool,
    batch: u32,
    lora: Option<PathBuf>,
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

    /// The running endpoint, if it is already serving this model.
    pub fn endpoint_for(&mut self, model_id: &str) -> Option<Endpoint> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        (r.model_id == model_id).then(|| Endpoint { port: r.port, key: r.key.clone(), ctx: r.ctx, extra: serde_json::Value::Null, cloud: None })
    }

    /// Whether the running server already matches this launch exactly.
    pub fn matches(&mut self, spec: &LaunchSpec<'_>) -> bool {
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
            })
    }

    /// The running model was started with its image encoder.
    pub fn sees(&mut self) -> bool {
        self.alive() && self.running.as_ref().is_some_and(|r| r.vision)
    }

    /// Starts the server for this model, replacing any other running model
    /// (or the same one launched with other limits).
    pub async fn ensure(&mut self, spec: LaunchSpec<'_>, job: Option<&JobRef>) -> Result<Endpoint, String> {
        if self.matches(&spec) {
            let r = self.running.as_ref().unwrap();
            return Ok(Endpoint { port: r.port, key: r.key.clone(), ctx: r.ctx, extra: serde_json::Value::Null, cloud: None });
        }
        self.stop().await;

        let port = free_port()?;
        let key = uuid::Uuid::new_v4().simple().to_string();
        let log = std::fs::File::create(spec.log).map_err(|e| e.to_string())?;
        let log_err = log.try_clone().map_err(|e| e.to_string())?;

        let mut cmd = Command::new(spec.exe);
        cmd.arg("-m").arg(spec.model_path)
            .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--api-key", &key])
            .args(["-c", &spec.ctx.to_string(), "-ngl", &spec.gpu_layers.to_string()])
            // One conversation at a time gets the whole context window.
            .args(["-np", "1", "--jinja", "--no-webui"])
            .args(if spec.threads > 0 { vec!["-t".to_string(), spec.threads.to_string()] } else { vec![] })
            .args(if spec.batch > 0 { vec!["-b".to_string(), spec.batch.to_string()] } else { vec![] })
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
            quant: spec.quant.to_string(),
            ctx: spec.ctx,
            gpu_layers: spec.gpu_layers,
            threads: spec.threads,
            low_priority: spec.low_priority,
            vision: spec.mmproj.is_some(),
            batch: spec.batch,
            lora: spec.lora.map(Path::to_path_buf),
        });

        let endpoint = Endpoint { port, key, ctx: spec.ctx, extra: serde_json::Value::Null, cloud: None };
        if let Err(e) = self.wait_ready(&endpoint, spec.log).await {
            self.stop().await;
            return Err(e);
        }
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
pub async fn benchmark(ep: &Endpoint) -> Result<f64, String> {
    let body = serde_json::json!({
        "messages": [{ "role": "user", "content": "Write three short sentences about the ocean." }],
        "max_tokens": 128,
        "temperature": 0.7,
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
    if let Some(tps) = resp.pointer("/timings/predicted_per_second").and_then(|v| v.as_f64()) {
        return Ok((tps * 10.0).round() / 10.0);
    }
    let tokens = resp.pointer("/usage/completion_tokens").and_then(|v| v.as_f64()).unwrap_or(0.0);
    Ok((tokens / started.elapsed().as_secs_f64() * 10.0).round() / 10.0)
}

#[cfg(test)]
mod platform_tests {
    use super::*;

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

    #[test]
    fn engine_builds_per_system() {
        assert_eq!(backend_named("windows", "x86_64", true).unwrap(), "vulkan-x64");
        assert_eq!(backend_named("linux", "x86_64", false).unwrap(), "linux-cpu-x64");
        assert_eq!(backend_named("macos", "aarch64", true).unwrap(), "macos-arm64");
        assert!(backend_named("freebsd", "x86_64", true).is_err());
    }
}

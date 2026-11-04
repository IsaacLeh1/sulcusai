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
    let nvidia = crate::hardware::cards().iter().any(|g| g.vendor == "NVIDIA" && !g.integrated);
    Ok(cuda_on_arm(std::env::consts::OS, std::env::consts::ARCH, nvidia).unwrap_or(backend_named(std::env::consts::OS, std::env::consts::ARCH, budget.vram > 0)?))
}

/// Linux on ARM with an NVIDIA chip (DGX Spark's GB10): llama.cpp's CUDA
/// build, since Vulkan leaves much of its speed unused. It uses the CUDA
/// libraries DGX OS ships with.
fn cuda_on_arm(os: &str, arch: &str, nvidia: bool) -> Option<&'static str> {
    (os == "linux" && arch == "aarch64" && nvidia).then_some("linux-cuda-arm64")
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
    CAPPED.lock().unwrap().insert(path.to_path_buf());
    Ok((out, err))
}

static CAPPED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);

/// Starts any server log past `LOG_CAP` over. Called by the loops that
/// already run while a server is loaded (perf.rs, speech.rs), so nothing
/// wakes for it while the app sits idle.
pub fn cap_logs() {
    for p in CAPPED.lock().unwrap().iter() {
        if std::fs::metadata(p).is_ok_and(|m| m.len() > LOG_CAP) {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(p) {
                let _ = f.set_len(0);
            }
        }
    }
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
/// ("Vulkan0", "NVIDIA GeForce RTX 4070"), in the engine's own order. Asked
/// once, and asked again after the PC is checked again ("Check again", a new
/// power source) or when a model ran on another card than the one picked by
/// name: the numbers are the engine's own and can change with the drivers.
static DEVICES: std::sync::Mutex<Option<(PathBuf, Vec<(String, String)>)>> = std::sync::Mutex::new(None);

/// Forgets which devices the engines see, so the next model asks again.
pub fn forget_devices() {
    *DEVICES.lock().unwrap() = None;
}

/// The devices the engine listed last (None: not asked since the PC was
/// last checked).
pub fn devices_seen() -> Option<Vec<(String, String)>> {
    DEVICES.lock().unwrap().as_ref().map(|(_, d)| d.clone())
}

/// The engine sees a graphics card it may use: a device that isn't the
/// integrated graphics. None until it was asked.
pub fn sees_a_card(devices: &[(String, String)], cards: &[crate::hardware::Gpu]) -> bool {
    devices.iter().any(|(_, name)| !cards.iter().any(|c| c.integrated && same_card(name, &c.name)))
}

fn same_card(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    a == b || a.contains(&b) || b.contains(&a)
}

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

/// Of the devices an engine sees, the dedicated graphics cards (number and
/// name), when the PC also has integrated graphics the engine could
/// otherwise use. Matched by name against the cards Windows reports.
fn dedicated_of(devices: &[(String, String)], cards: &[crate::hardware::Gpu]) -> Vec<(String, String)> {
    let is_integrated = |name: &str| built_in(name, cards);
    let is_dedicated = |name: &str| cards.iter().any(|c| !c.integrated && same_card(name, &c.name));
    let mut dedicated: Vec<(String, String)> = devices.iter().filter(|(_, n)| is_dedicated(n) && !is_integrated(n)).cloned().collect();
    // The engine names the card differently from Windows: everything but
    // the built-in graphics, which would put the model in system memory
    // (their memory is the PC's RAM).
    if dedicated.is_empty() && cards.iter().any(|c| !c.integrated) {
        dedicated = devices.iter().filter(|(_, n)| !is_integrated(n)).cloned().collect();
    }
    if dedicated.is_empty() || dedicated.len() == devices.len() {
        return Vec::new();
    }
    dedicated
}

/// A device that is built-in graphics: named like an integrated card Windows
/// reports, or, beside a separate card from another maker, Intel or AMD
/// graphics without a model of their own ("Intel(R) Arc(TM) Graphics" and
/// "AMD Radeon(TM) Graphics" are the processor's; Windows may call them
/// just "Intel(R) Graphics").
fn built_in(name: &str, cards: &[crate::hardware::Gpu]) -> bool {
    if cards.iter().any(|c| c.integrated && same_card(name, &c.name)) {
        return true;
    }
    let low = name.to_lowercase();
    let maker = |c: &crate::hardware::Gpu| format!("{} {}", c.vendor, c.name).to_lowercase();
    let separate_other = |m: &str| cards.iter().any(|c| !c.integrated && !maker(c).contains(m));
    (low.contains("intel") && separate_other("intel")) || ((low.contains("radeon(tm) graphics") || low == "amd radeon graphics" || crate::hardware::built_in_name(name)) && separate_other("amd"))
}

/// The devices `exe` (a llama.cpp program) sees, asked once until the PC is
/// checked again. A fresh list goes in the app log: when a model runs on the
/// processor, it says whether the engine saw the card at all.
async fn list_devices(exe: &Path) -> Vec<(String, String)> {
    let known = DEVICES.lock().unwrap().as_ref().filter(|(p, _)| p == exe).map(|(_, d)| d.clone());
    if let Some(d) = known {
        return d;
    }
    let mut cmd = Command::new(exe);
    cmd.arg("--list-devices").stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = match tokio::time::timeout(Duration::from_secs(20), cmd.output()).await {
        Ok(Ok(o)) => format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
        _ => String::new(),
    };
    let d = parse_devices(&out);
    crate::applog::line("engine", &devices_line(&d));
    *DEVICES.lock().unwrap() = Some((exe.to_path_buf(), d.clone()));
    d
}

/// The app log's line for what `--list-devices` found.
fn devices_line(d: &[(String, String)]) -> String {
    // A log line, not a message for the window (so not `.into()`, which
    // the translation check would pick up).
    if d.is_empty() {
        return String::from("Graphics devices the engines see: none (models run on the processor; if this PC has a graphics card, its driver may be missing or too old)");
    }
    format!("Graphics devices the engines see: {}", d.iter().map(|(id, name)| format!("{id} {name}")).collect::<Vec<_>>().join(", "))
}

/// The device(s) to run on: the main card (picked by the person, else the
/// dedicated card with the most memory, whose memory the model was fitted
/// to), found among the engine's devices by name. When the engine names it
/// differently, the dedicated cards when there's integrated graphics to
/// keep it off, as before; else the engine decides.
fn pick_devices(devices: &[(String, String)], cards: &[crate::hardware::Gpu], main: Option<&crate::hardware::Gpu>) -> Vec<(String, String)> {
    if let Some(card) = main.filter(|c| !c.integrated) {
        let integrated = |name: &str| cards.iter().any(|c| c.integrated && same_card(name, &c.name));
        // The exact name first: "RTX 4060" mustn't pick a "RTX 4060 Ti" beside it.
        let exact = devices.iter().find(|(_, n)| n.eq_ignore_ascii_case(&card.name) && !integrated(n));
        if let Some(d) = exact.or_else(|| devices.iter().find(|(_, n)| same_card(n, &card.name) && !integrated(n))) {
            return vec![d.clone()];
        }
    }
    dedicated_of(devices, cards)
}

/// The main graphics card's name, which a running model is tied to.
fn main_card() -> Option<String> {
    crate::hardware::dedicated().map(|g| g.name)
}

/// The device(s) for `exe`: with `spread`, every card models are spread
/// over when there are several (`hardware::model_cards`; llama.cpp splits
/// the layers by their free memory), else `pick_devices`.
pub(crate) async fn dedicated_devices(exe: &Path, spread: bool) -> Vec<(String, String)> {
    let devices = list_devices(exe).await;
    let cards = crate::hardware::cards();
    let several = crate::hardware::model_cards(&cards, crate::hardware::main_choice().as_deref());
    if spread && several.len() > 1 {
        let found = pick_several(&devices, &cards, &several);
        if found.len() >= several.len() {
            return found;
        }
    }
    pick_devices(&devices, &cards, crate::hardware::dedicated().as_ref())
}

/// The engine's devices for several cards: each device named like one of
/// them (two identical cards are two devices of the same name), never the
/// built-in graphics.
fn pick_several(devices: &[(String, String)], cards: &[crate::hardware::Gpu], several: &[&crate::hardware::Gpu]) -> Vec<(String, String)> {
    devices.iter().filter(|(_, n)| several.iter().any(|c| same_card(n, &c.name)) && !built_in(n, cards)).cloned().collect()
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

/// The main card's Vulkan number ("0"), when known for sure: the engine
/// listed every card.
fn dedicated_indexes() -> Option<String> {
    let (_, devices) = DEVICES.lock().unwrap().clone()?;
    let cards = crate::hardware::cards();
    if devices.len() != cards.len() || std::env::var_os("GGML_VK_VISIBLE_DEVICES").is_some() {
        return None;
    }
    // The card picked, else the biggest dedicated one: these engines run on
    // one card and were fitted to it (with two, the first listed can be the
    // smaller). Not found by name: the dedicated ones, as before.
    let picked: Vec<String> = pick_devices(&devices, &cards, crate::hardware::dedicated().as_ref()).iter().filter_map(|(id, _)| id.strip_prefix("Vulkan").map(str::to_string)).collect();
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
            list_devices(&exe).await;
        }
    }
    dedicated_indexes().is_some()
}

/// Where a loaded model really runs, from the engine's own log (llama.cpp
/// at verbosity 4): the layers it put on a graphics card, the devices it
/// used, and how much of the model's weights went to cards and to system
/// memory. What the app planned can differ: an engine that doesn't see the
/// card (an old driver) runs everything on the processor.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Placed {
    /// "offloaded 37/37 layers to GPU": layers on a card, layers in all.
    pub layers: Option<(u32, u32)>,
    /// The devices it used: ("Vulkan0", "NVIDIA GeForce RTX 5070").
    pub devices: Vec<(String, String)>,
    /// The weights on graphics cards and in system memory (MiB), from the
    /// "model buffer size" lines; both 0 when there were none.
    pub card_mib: f64,
    pub memory_mib: f64,
}

impl Placed {
    /// Layers really on a graphics card: none when no weights went to one,
    /// whatever the count says (an engine with no card to use still counts
    /// the layers it was asked for).
    pub fn on_card(&self) -> Option<u32> {
        let buffers = self.card_mib + self.memory_mib > 0.0;
        if buffers && self.card_mib == 0.0 {
            return Some(0);
        }
        match self.layers {
            Some((n, _)) => Some(if self.devices.is_empty() && !buffers { 0 } else { n }),
            // No count but weights in memory only (a processor build): none.
            None if buffers => Some(0),
            None => None,
        }
    }

    pub fn total(&self) -> Option<u32> {
        self.layers.map(|(_, all)| all)
    }

    /// The card's name as the engine reported it ("NVIDIA GeForce RTX 5070"),
    /// when it used one.
    pub fn device(&self) -> Option<String> {
        if self.devices.is_empty() || self.on_card() == Some(0) {
            return None;
        }
        Some(self.devices.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(" + "))
    }

    /// For the app log: "running 37/37 layers on NVIDIA GeForce RTX 5070".
    pub fn describe(&self) -> String {
        match (self.on_card(), self.total()) {
            (Some(0), Some(all)) => format!("running 0/{all} layers on the graphics card: all on the processor"),
            (Some(0), None) => "running on the processor only".to_string(),
            (Some(n), Some(all)) if n >= all => format!("running {n}/{all} layers on {}", self.device().unwrap_or_else(|| "the graphics card".into())),
            (Some(n), Some(all)) => format!("running {n}/{all} layers on {}, the rest on the processor", self.device().unwrap_or_else(|| "the graphics card".into())),
            _ => "the engine's log didn't say where it runs".to_string(),
        }
    }
}

/// Reads where the model went from an engine log. When the log holds more
/// than one load (the engine tries settings before the real load), the last.
pub fn parse_placed(log: &str) -> Placed {
    let mut p = Placed::default();
    for line in log.lines() {
        if let Some(rest) = line.split_once("using device ").map(|(_, r)| r) {
            // A new load: what came before was a trial.
            if p.layers.is_some() || p.card_mib + p.memory_mib > 0.0 {
                p = Placed::default();
            }
            if let Some(d) = device_of(rest) {
                if !p.devices.contains(&d) {
                    p.devices.push(d);
                }
            }
        } else if let Some(rest) = line.split_once("offloaded ").map(|(_, r)| r).filter(|r| r.contains(" layers to GPU")) {
            let counts = rest.split_whitespace().next().and_then(|c| c.split_once('/'));
            if let Some((n, all)) = counts.and_then(|(n, all)| Some((n.parse().ok()?, all.parse().ok()?))) {
                p.layers = Some((n, all));
            }
        } else if let Some((before, after)) = line.split_once(" model buffer size") {
            let Some(name) = before.split_whitespace().last() else { continue };
            let mib = after.trim_start_matches([' ', '=']).split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            // System memory: the processor's buffers and the card's pinned
            // host memory ("CPU_Mapped", "CPU_REPACK", "Vulkan_Host").
            if name.starts_with("CPU") || name.ends_with("_Host") {
                p.memory_mib += mib;
            } else {
                p.card_mib += mib;
            }
        }
    }
    p
}

/// "Vulkan0 (NVIDIA GeForce RTX 5070) (0000:01:00.0) - 11175 MiB free" →
/// ("Vulkan0", "NVIDIA GeForce RTX 5070"). Names can hold brackets
/// ("Intel(R) UHD Graphics 770").
fn device_of(rest: &str) -> Option<(String, String)> {
    let (id, after) = rest.trim().split_once(' ')?;
    let after = after.trim_start().strip_prefix('(')?;
    let mut depth = 1;
    for (i, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let name = after[..i].trim();
                    return (!id.is_empty() && !name.is_empty()).then(|| (id.to_string(), name.to_string()));
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds a program by file name anywhere under `dir`.
pub fn find_exe(dir: &Path, name: &str) -> Option<PathBuf> {
    // Most are right here: looked for first, before a walk through every
    // folder (a Python environment beside it has tens of thousands of files).
    if dir.join(name).is_file() {
        return Some(dir.join(name));
    }
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
    let exe = ensure_unpacked(paths, &dir, asset, SERVER_EXE, cancel, progress).await?;
    // The CUDA build needs CUDA 13's libraries: DGX OS has them, other
    // Linux systems on ARM may not, so llama.cpp's own copy comes along.
    if backend == CUDA_ARM && !system_has_cudart() {
        if let Some(runtime) = spec.assets.get(CUDA_ARM_RUNTIME) {
            ensure_unpacked(paths, &runtime_dir_name(&dir), runtime, CUDART, cancel, |_, _| {}).await?;
        }
    }
    Ok(exe)
}

const CUDA_ARM: &str = "linux-cuda-arm64";
/// llama.cpp's `cudart-...-arm64` archive, in the catalog beside the build.
const CUDA_ARM_RUNTIME: &str = "linux-cuda-arm64-runtime";
const CUDART: &str = "libcudart.so.13";

fn runtime_dir_name(engine_dir: &str) -> String {
    engine_dir.replace(CUDA_ARM, "cuda-runtime-arm64")
}

/// CUDA 13's runtime where the system keeps its libraries.
fn system_has_cudart() -> bool {
    ["/usr/local/cuda/lib64", "/usr/local/cuda/targets/sbsa-linux/lib", "/usr/lib/aarch64-linux-gnu"].iter().any(|d| Path::new(d).join(CUDART).exists())
}

/// The folder of CUDA libraries unpacked beside a CUDA engine, if any: the
/// engine finds them through `LD_LIBRARY_PATH`.
fn cuda_runtime_beside(exe: &Path) -> Option<PathBuf> {
    let engine_dir = exe.ancestors().find(|a| a.file_name().is_some_and(|n| n.to_string_lossy().ends_with(CUDA_ARM)))?;
    let name = runtime_dir_name(&engine_dir.file_name()?.to_string_lossy());
    let lib = find_exe(&engine_dir.with_file_name(name), CUDART)?;
    lib.parent().map(Path::to_path_buf)
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
    // Recorded before anything runs from it, so every start can tell a file
    // changed later. A failure only leaves the folder unchecked.
    let (p, d, a, s) = (paths.clone(), dir_name.to_string(), asset.clone(), staging.clone());
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || crate::integrity::record_unpacked(&p, &d, &a, &s)).await {
        crate::applog::line("security", &e);
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&staging, &dir).map_err(|e| e.to_string())?;
    std::fs::remove_file(&zip_path).ok();
    let found = find_exe(&dir, exe).ok_or_else(|| format!("The engine download didn't contain {exe}."))?;
    // A new version is in place: the one it replaced isn't needed.
    crate::cleanup::remove_replaced_engines();
    Ok(found)
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

/// Unpacks a zip where the app would (tests that place a package by hand).
#[cfg(test)]
pub(crate) fn unzip_for_test(zip_path: &Path, into: &Path) -> Result<(), String> {
    unzip(zip_path, into)
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

/// The DLLs in a zip (or a Python wheel, which is one), wherever they are
/// in it, put side by side in `into`; the rest (headers, import libraries,
/// Python files) is left out.
pub fn unzip_libraries(zip_path: &Path, into: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Couldn't unpack {}: {e}", zip_path.display()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name().and_then(|p| p.file_name().map(|n| n.to_owned())) else { continue };
        if entry.is_dir() || !name.to_string_lossy().to_lowercase().ends_with(".dll") {
            continue;
        }
        let mut dest = std::fs::File::create(into.join(&name)).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut dest).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
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
    /// A direction from the Lens added to its layers while it answers.
    pub steer: Option<&'a crate::steer::Steer>,
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
    /// K and V kept as q4_0 (chosen in Advanced or a model's settings):
    /// smaller again, a little less exact.
    pub kv_q4: bool,
    /// Flash attention on or off (`-fa`), when chosen; else llama.cpp decides.
    pub flash: Option<bool>,
    /// Most tokens a reply may think before it's told to answer
    /// (`--reasoning-budget`): `think_budget` in lib.rs. None: no limit.
    pub think_budget: Option<u32>,
    /// Spread over every card models use (`catalog::fits_biggest_card`
    /// says it doesn't fit the biggest alone); else on the main card.
    pub spread: bool,
    /// An embedding model (the documents library's): it turns text into
    /// numbers (`/v1/embeddings`, last-token pooling) instead of chatting.
    pub embedding: bool,
}


/// How many layers a model file has for predicting several tokens at once
/// (multi-token prediction: `{arch}.nextn_predict_layers`, kept in the
/// "MTP" GGUFs), 0 without.
/// Replies a chat model works on at once here (`catalog::parallel_slots`),
/// by the memory of the cards models use.
fn parallel_here() -> u32 {
    let cards = crate::hardware::cards();
    let chosen = crate::hardware::main_choice();
    let vram = crate::hardware::model_cards(&cards, chosen.as_deref()).iter().map(|g| g.vram_bytes).sum();
    crate::catalog::parallel_slots(vram)
}

pub fn mtp_layers(path: &Path) -> u32 {
    crate::gguf::read(path)
        .ok()
        .and_then(|m| {
            let arch = m.get("general.architecture")?.str()?.to_string();
            m.get(&format!("{arch}.nextn_predict_layers"))?.int()
        })
        .map_or(0, |n| n.max(0) as u32)
}

/// Guessing ahead, checked by the model in one pass: from text already in
/// the conversation (n-gram lookup), and with the model's own MTP layers
/// when it has them (2 tokens at a time). Measured on an RTX 5070 with
/// Qwen3.5 9B: 94 to 111-117 tokens/s writing a story, 94 to 136-143
/// writing code; 3 at a time guessed wrong too often (96-101 on the story).
fn spec_args(model: &Path, gpu_layers: u32) -> Vec<&'static str> {
    if gpu_layers > 0 && mtp_layers(model) > 0 {
        vec!["--spec-type", "draft-mtp,ngram-simple", "--spec-draft-n-max", "2"]
    } else {
        vec!["--spec-type", "ngram-simple"]
    }
}

/// Said at the end of thinking that ran out of its budget.
const THINK_BUDGET_MESSAGE: &str = "\n\nI've thought about this enough. Time to answer.\n";

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
    steer: Option<crate::steer::Steer>,
    cpu_moe: Option<u32>,
    kv_q8: bool,
    kv_q4: bool,
    flash: Option<bool>,
    think_budget: Option<u32>,
    spread: bool,
    /// Where it really runs, from the engine's log once it loaded.
    placed: Placed,
    /// The main graphics card when it started (`hardware::dedicated`): picking
    /// another one starts it again there.
    card: Option<String>,
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
    /// The layers planned for the graphics card.
    pub gpu_layers: Option<u32>,
    /// What the engine reported once it loaded: layers really on a graphics
    /// card, the model's layers in all, and the card's name (None: not known).
    pub layers_on_card: Option<u32>,
    pub layers_total: Option<u32>,
    pub device: Option<String>,
    /// The model used before, still loaded beside it.
    pub kept: Option<String>,
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
    /// The model used before, kept loaded beside it while the card has room
    /// for both (`keep_next`): asked for again, it answers at once.
    kept: Option<Running>,
    /// The next model started may leave the one running now loaded: set by
    /// the caller once it has checked the card has room for both.
    keep_next: bool,
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
    steer: Option<crate::steer::Steer>,
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
        let kept = self.kept_model();
        let r = self.running.as_ref();
        EngineStatus {
            model_id: r.map(|r| r.model_id.clone()),
            quant: r.map(|r| r.quant.clone()),
            ctx: r.map(|r| r.ctx),
            gpu_layers: r.map(|r| r.gpu_layers),
            layers_on_card: r.and_then(|r| r.placed.on_card()),
            layers_total: r.and_then(|r| r.placed.total()),
            device: r.and_then(|r| r.placed.device()),
            kept,
        }
    }

    /// The running model: what was planned for it (GPU layers, experts kept
    /// in system memory) and where the engine really put it.
    pub fn placement(&mut self) -> Option<(String, String, u32, Option<u32>, Placed)> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        Some((r.model_id.clone(), r.quant.clone(), r.gpu_layers, r.cpu_moe, r.placed.clone()))
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
    /// Steered by this direction (or none) too: a steered model and the
    /// plain one answer differently.
    pub fn endpoint_steered(&mut self, model_id: &str, lora: Option<&Path>, steer: Option<&crate::steer::Steer>) -> Option<Endpoint> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        (r.model_id == model_id && r.lora.as_deref() == lora && r.steer.as_ref() == steer).then(|| r.endpoint())
    }

    /// The model running now, whatever it is: its id and endpoint.
    pub fn current(&mut self) -> Option<(String, Endpoint)> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        Some((r.model_id.clone(), r.endpoint()))
    }

    /// What the running model was asked for (a model, or a taught model's
    /// own id): what to ask for to start it again elsewhere.
    pub fn asked(&mut self) -> Option<String> {
        if !self.alive() {
            return None;
        }
        self.running.as_ref().map(|r| r.asked.clone())
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
                    && r.steer.as_ref() == spec.steer
                    && r.cpu_moe == tuning.cpu_moe
                    && r.kv_q8 == tuning.kv_q8
                    && r.kv_q4 == tuning.kv_q4
                    && r.flash == tuning.flash
                    && r.think_budget == tuning.think_budget
                    && r.spread == tuning.spread
                    && r.card == main_card()
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
        crate::quiet::stir();
        let started = Instant::now();
        match self.ensure_tuned_once(spec, tuning, job).await {
            // Gone at once with nothing in its log: something hooking into
            // new GPU programs (a game overlay) or a driver hiccup at start,
            // which a second start gets past.
            Err(e) if started.elapsed() < Duration::from_secs(8) && e.trim_end() == STOPPED_LOADING => {
                crate::applog::line("engine", "The engine stopped as it started, with nothing in its log; starting it once more");
                self.ensure_tuned_once(spec, tuning, job).await
            }
            r => r,
        }
    }

    async fn ensure_tuned_once(&mut self, spec: LaunchSpec<'_>, tuning: Tuning, job: Option<&JobRef>) -> Result<Endpoint, String> {
        if self.matches_tuned(&spec, tuning) {
            return Ok(self.running.as_ref().unwrap().endpoint());
        }
        let keep = std::mem::take(&mut self.keep_next);
        // The model kept from before, as launched: the two change places.
        let kept_alive = self.kept.as_mut().is_some_and(|r| matches!(r.child.try_wait(), Ok(None)));
        if kept_alive {
            std::mem::swap(&mut self.running, &mut self.kept);
            if self.matches_tuned(&spec, tuning) {
                crate::applog::line("engine", &format!("{} was still loaded: answering at once", spec.model_id));
                return Ok(self.running.as_ref().unwrap().endpoint());
            }
            std::mem::swap(&mut self.running, &mut self.kept);
        } else {
            self.kept = None;
        }
        // Another model, with room on the card for both: the one running now
        // stays loaded (the one kept before it goes).
        let other = self.running.as_ref().is_some_and(|r| r.model_id != spec.model_id || r.lora.as_deref() != spec.lora || r.steer.as_ref() != spec.steer);
        if keep && other && self.alive() {
            self.drop_kept().await;
            self.kept = self.running.take();
            crate::applog::line("engine", "Keeping the model used before loaded beside the new one (the card has room for both)");
        } else {
            // No room for three: the one kept from before goes first.
            self.drop_kept().await;
        }
        // The same model with other limits (mode, heat, its picture reader)
        // keeps its address and key, so a reply holding them carries on.
        let same = |model: &str, lora: Option<&Path>, steer: Option<&crate::steer::Steer>| model == spec.model_id && lora == spec.lora && steer == spec.steer;
        let relaunch = self.running.as_ref().filter(|r| same(&r.model_id, r.lora.as_deref(), r.steer.as_ref())).map(|r| (r.port, r.key.clone()));
        self.stop_running().await;

        // Coming back from a pause: the same address too, if it's still free.
        let back = self.paused.take().filter(|p| same(&p.base, p.lora.as_deref(), p.steer.as_ref())).map(|p| (p.port, p.key));
        let (port, key) = match relaunch.or(back).filter(|(port, _)| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok()) {
            Some(p) => p,
            None => (free_port()?, uuid::Uuid::new_v4().simple().to_string()),
        };
        let (log, log_err) = capped_log(spec.log).map_err(|e| e.to_string())?;

        // On a PC with integrated graphics too, the model goes on the
        // dedicated card. The devices are listed (and logged) for a model on
        // the processor too: whether the engine sees the card explains it.
        // Out of the way, the card isn't touched at all, not even to list it
        // (a CUDA start alone holds graphics memory).
        let card_off = spec.gpu_layers == 0 && crate::perf::away_now();
        let dedicated = if card_off { Vec::new() } else { dedicated_devices(spec.exe, tuning.spread).await };
        let devices: Vec<(String, String)> = if spec.gpu_layers > 0 { dedicated } else { Vec::new() };
        let ids: Vec<&str> = devices.iter().map(|(id, _)| id.as_str()).collect();
        let mut cmd = Command::new(spec.exe);
        if let Some(libs) = cuda_runtime_beside(spec.exe) {
            let mut paths = vec![libs];
            paths.extend(std::env::var_os("LD_LIBRARY_PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("LD_LIBRARY_PATH", joined);
            }
        }
        cmd.args(if card_off {
            vec!["--device".to_string(), "none".to_string()]
        } else if ids.is_empty() {
            vec![]
        } else {
            vec!["--device".to_string(), ids.join(",")]
        });
        // Verbosity 4 puts llama.cpp's own loading lines in the log ("using
        // device …", "offloaded 37/37 layers to GPU"): where the model really
        // went. The default (3) leaves them out.
        cmd.args(["-lv", "4"]);
        // Experts kept in system memory: every layer on the card otherwise.
        let layers = match tuning.cpu_moe {
            Some(n) => vec!["-ngl".to_string(), "999".to_string(), "--n-cpu-moe".to_string(), n.to_string()],
            None => vec!["-ngl".to_string(), spec.gpu_layers.to_string()],
        };
        // Machines linked for models: their rpc-servers take a share of the layers.
        let linked = crate::hardware::linked();
        if !tuning.embedding && !linked.is_empty() {
            cmd.args(["--rpc", &linked.iter().map(|m| m.addr.as_str()).collect::<Vec<_>>().join(",")]);
        }
        cmd.arg("-m").arg(spec.model_path)
            .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--api-key", &key])
            .args(["-c", &spec.ctx.to_string()])
            .args(layers)
            // Several replies at once (chats on the same model answering side
            // by side, continuous batching), sharing one context window: a
            // reply alone keeps its full speed (Qwen3.5 9B with MTP, 112-136
            // tokens/s with 1 slot or 2), and two together finish as fast as
            // one after the other did, both streaming. An embedding model
            // reads one passage at a time.
            .args(if tuning.embedding { vec!["-np".to_string(), "1".to_string()] } else { vec!["-np".to_string(), parallel_here().to_string(), "-kvu".to_string()] })
            .args(["--jinja", "--no-webui"])
            // After old messages are dropped to fit, the parts that didn't
            // change are reused from the cache, not read again.
            .args(if tuning.embedding { vec![] } else { vec!["--cache-reuse", "256"] })
            // A passage is read in one batch, so it can't be longer than one.
            .args(if tuning.embedding { vec!["--embedding", "--pooling", "last", "-ub", "4096", "-b", "4096"] } else { vec![] })
            // Guessing ahead from text already in the conversation (n-gram
            // lookup, no extra model): a reply that repeats it (code edited,
            // text quoted or revised) comes faster, the rest as fast as before.
            // Measured on a code edit: Qwen3 4B 162 to 203 tokens/s, Qwen3.5 9B
            // 88 to 105, gpt-oss 20B 29 to 47; open writing unchanged. The
            // "cache" kind made both slower, "mod" helped less.
            .args(if tuning.embedding { vec![] } else { spec_args(spec.model_path, spec.gpu_layers) })
            .args(if spec.threads > 0 { vec!["-t".to_string(), spec.threads.to_string()] } else { vec![] })
            .args(if spec.batch > 0 { vec!["-b".to_string(), spec.batch.to_string()] } else { vec![] })
            .args(if tuning.kv_q4 {
                vec!["-ctk", "q4_0", "-ctv", "q4_0"]
            } else if tuning.kv_q8 {
                vec!["-ctk", "q8_0", "-ctv", "q8_0"]
            } else {
                vec![]
            })
            .args(match tuning.flash {
                Some(true) => vec!["-fa", "on"],
                Some(false) => vec!["-fa", "off"],
                None => vec![],
            })
            .args(match tuning.think_budget {
                Some(n) => vec!["--reasoning-budget".to_string(), n.to_string(), "--reasoning-budget-message".to_string(), THINK_BUDGET_MESSAGE.to_string()],
                None => vec![],
            })
            // With the model on the card, its file isn't also kept in system
            // memory: on Windows llama.cpp keeps a mapped file in memory after
            // copying it to the card, so RAM filled by the model's size for
            // nothing. Memory is for what the card can't hold.
            .args(if spec.gpu_layers > 0 { vec!["--load-mode", "none"] } else { vec![] })
            .args(spec.lora.map(|p| vec![std::ffi::OsString::from("--lora"), p.into()]).unwrap_or_default())
            .args(spec.steer.map(crate::steer::Steer::args).unwrap_or_default())
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
            steer: spec.steer.cloned(),
            cpu_moe: tuning.cpu_moe,
            kv_q8: tuning.kv_q8,
            kv_q4: tuning.kv_q4,
            flash: tuning.flash,
            think_budget: tuning.think_budget,
            spread: tuning.spread,
            placed: Placed::default(),
            card: main_card(),
        });

        let endpoint = Endpoint { port, key, ctx: spec.ctx, extra: serde_json::Value::Null, cloud: None };
        let started = Instant::now();
        let what = format!(
            "{} ({}, {} context, {}{}{}{}{})",
            spec.model_id,
            spec.quant,
            spec.ctx,
            match tuning.cpu_moe {
                Some(n) => format!("planned every layer on the graphics card, the experts of {n} in memory"),
                None => format!("planned {} layers on the graphics card", spec.gpu_layers),
            },
            if tuning.kv_q4 { ", small cache (q4)" } else if tuning.kv_q8 { ", compressed cache" } else { "" },
            tuning.think_budget.map(|n| format!(", thinking up to {n} tokens")).unwrap_or_default(),
            if devices.is_empty() { String::new() } else { format!(", device {}", devices.iter().map(|(id, name)| format!("{id} {name}")).collect::<Vec<_>>().join(",")) },
            if spec.mmproj.is_some() { ", sees pictures" } else { "" }
        );
        if let Err(e) = self.wait_ready(&endpoint, spec.log).await {
            crate::applog::line("engine", &format!("Couldn't load {what}: {e}"));
            self.stop().await;
            return Err(e);
        }
        let placed = read_placed(spec.log).await;
        crate::applog::line("engine", &format!("Loaded {what} in {:.1} s; {}", started.elapsed().as_secs_f64(), placed.describe()));
        // The card picked by name isn't the one the engine used under that
        // number (drivers renumber cards): the devices are asked again next time.
        if !devices.is_empty() && !placed.devices.is_empty() && !placed.devices.iter().any(|(_, n)| devices.iter().any(|(_, want)| same_card(n, want))) {
            crate::applog::line("engine", &format!("The engine used {} instead of {}; it will look for the cards again", placed.devices.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", "), devices.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")));
            forget_devices();
        }
        if let Some(r) = self.running.as_mut() {
            r.placed = placed;
        }
        Ok(endpoint)
    }

    async fn wait_ready(&mut self, ep: &Endpoint, log: &Path) -> Result<(), String> {
        let client = net::local_client();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(300) {
            if !self.alive() {
                return Err(format!("{STOPPED_LOADING}\n{}", log_tail(log, 12)));
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
        for mut r in [self.running.take(), self.kept.take()].into_iter().flatten() {
            r.child.start_kill().ok();
            let _ = tokio::time::timeout(Duration::from_secs(5), r.child.wait()).await;
        }
    }

    /// Stops the running model only (one kept from before stays).
    async fn stop_running(&mut self) {
        if let Some(mut r) = self.running.take() {
            r.child.start_kill().ok();
            let _ = tokio::time::timeout(Duration::from_secs(5), r.child.wait()).await;
        }
    }

    /// Stops the model kept from before, if any (its memory is wanted).
    pub async fn drop_kept(&mut self) {
        if let Some(mut r) = self.kept.take() {
            r.child.start_kill().ok();
            let _ = tokio::time::timeout(Duration::from_secs(5), r.child.wait()).await;
        }
    }

    /// The model kept loaded from before: its id, if it's still running.
    pub fn kept_model(&mut self) -> Option<String> {
        if self.kept.as_mut().is_some_and(|r| !matches!(r.child.try_wait(), Ok(None))) {
            self.kept = None;
        }
        self.kept.as_ref().map(|r| r.model_id.clone())
    }

    /// Whether the next model started may leave the running one loaded.
    pub fn keep_next(&mut self, keep: bool) {
        self.keep_next = keep;
    }

    /// Stops the model for a while (a picture needs the graphics card), keeping
    /// its address for when it's started again. What was paused, if anything.
    pub async fn pause(&mut self) -> Option<Paused> {
        if !self.alive() {
            return None;
        }
        let r = self.running.as_ref()?;
        let p = Paused { model_id: r.asked.clone(), vision: r.vision, base: r.model_id.clone(), lora: r.lora.clone(), steer: r.steer.clone(), port: r.port, key: r.key.clone() };
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

/// Background work on the loaded model (the mail report, meeting notes, a
/// translation, a helper's questions): while one is held, a chat doesn't
/// stop or relaunch that model under it, and it isn't removed.
static HOLDS: std::sync::Mutex<Vec<(u64, String)>> = std::sync::Mutex::new(Vec::new());
static NEXT_HOLD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[must_use = "the model is held only while this lives"]
pub struct Hold(u64);

impl Hold {
    pub fn new(key: &str) -> Hold {
        let id = NEXT_HOLD.fetch_add(1, Ordering::Relaxed);
        HOLDS.lock().unwrap().push((id, key.to_string()));
        Hold(id)
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        HOLDS.lock().unwrap().retain(|(i, _)| *i != self.0);
    }
}

/// Background jobs holding the model with this key.
pub fn held(key: &str) -> usize {
    HOLDS.lock().unwrap().iter().filter(|(_, k)| k == key).count()
}

/// Requests for another model than the one running, waiting their turn:
/// one model runs at a time, and they load it in the order they asked.
static LINE: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

/// A place in that line, kept while this lives.
#[must_use = "the place in line is kept only while this lives"]
pub struct Ticket(u64);

impl Ticket {
    pub fn new() -> Ticket {
        let id = NEXT_HOLD.fetch_add(1, Ordering::Relaxed);
        LINE.lock().unwrap().push(id);
        Ticket(id)
    }

    /// Its place: 1 is next.
    pub fn place(&self) -> usize {
        LINE.lock().unwrap().iter().position(|i| *i == self.0).map_or(1, |i| i + 1)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        LINE.lock().unwrap().retain(|i| *i != self.0);
    }
}

/// How many are waiting in line now.
pub fn waiting() -> usize {
    LINE.lock().unwrap().len()
}

/// What a request for a model is doing before it's ready.
#[derive(Debug, Clone, PartialEq)]
pub enum Wait {
    /// Loading it into memory.
    Loading,
    /// In line: its place (1 is next), and the model the replies ahead of it use.
    Line(usize, String),
}

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

pub const STOPPED_LOADING: &str = "The engine stopped while loading the model.";

pub fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

/// Where the model went, from its log. The engine writes its log from a
/// thread of its own, so it's read again for a moment if the loading lines
/// aren't there yet.
async fn read_placed(log: &Path) -> Placed {
    let read = || parse_placed(&String::from_utf8_lossy(&std::fs::read(log).unwrap_or_default()));
    for _ in 0..4 {
        let p = read();
        if p.layers.is_some() || p.card_mib + p.memory_mib > 0.0 {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    read()
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
mod platform_tests;

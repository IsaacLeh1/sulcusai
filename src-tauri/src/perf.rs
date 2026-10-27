// SPDX-License-Identifier: AGPL-3.0-only
//! How much of the PC the app may use, and keeping it from overheating.
//!
//! - Cool & quiet: half the processor, less graphics memory, low priority,
//!   models unloaded sooner. Used automatically on battery (optional).
//! - Balanced: the defaults.
//! - Turbo: the user picks the limits, but never past the ceilings, which
//!   always leave Windows room to run.
//!
//! The heat guard pauses between steps while the PC is hot. A critical
//! limit stays on in every mode.
//!
//! Adaptive cooling (a switch, on by default) watches the temperatures and
//! steps the app down as the PC heats up: fewer processor threads when the
//! processor is hot, fewer layers on the graphics card when it is, and short
//! pauses between steps. It steps back up once the PC has cooled.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::catalog::GIB;
use crate::hardware::Hardware;
use crate::{db, AppStateRef};

const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Cool,
    #[default]
    Balanced,
    Turbo,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Turbo {
    pub threads: usize,
    /// Share of graphics memory, percent.
    pub gpu_percent: u32,
    /// System memory for models, GB.
    pub ram_gb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PerfSettings {
    pub mode: Mode,
    pub turbo: Option<Turbo>,
    pub cool_on_battery: bool,
    pub heat_guard: bool,
    pub adaptive: bool,
    /// How much of the model may run on the graphics card, percent (the
    /// rest runs on the processor; 0 = processor only). Set at the top of
    /// a chat.
    pub gpu_limit: u8,
    /// Free the graphics card (unload the chat model) after this many idle
    /// minutes, 0 for never; None for the mode's own (Cool 5, Balanced 10,
    /// Turbo never).
    pub unload_after_mins: Option<u64>,
    /// Free it right after each reply (a few seconds after it was last
    /// used, so a reply's steps don't reload it), whatever the minutes say.
    #[serde(default)]
    pub unload_right_away: bool,
    /// Switching models, the one used before stays loaded while the card
    /// has room for both (going back to it needs no loading).
    pub keep_two: bool,
}

impl Default for PerfSettings {
    fn default() -> Self {
        PerfSettings { mode: Mode::Balanced, turbo: None, cool_on_battery: true, heat_guard: true, adaptive: true, gpu_limit: 100, unload_after_mins: None, unload_right_away: false, keep_two: true }
    }
}

/// The most the user may give the app: what the PC has, minus what
/// Windows and the display need.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Ceiling {
    pub threads: usize,
    pub gpu_percent: u32,
    pub ram_gb: f64,
    pub has_gpu: bool,
}

pub fn ceiling(hw: &Hardware) -> Ceiling {
    let vram = hw.model_vram();
    // Keep at least 400 MB of graphics memory for the screen.
    let gpu_percent = if vram > 0 { (((vram.saturating_sub(400 * MIB)) as f64 / vram as f64) * 100.0).floor().min(95.0) as u32 } else { 0 };
    // Windows needs at least 3 GB (or 15%) to stay usable.
    let keep = (hw.ram_total as f64 * 0.15).max(3.0 * GIB as f64);
    let ram_gb = ((hw.ram_total as f64 - keep).max(1.0 * GIB as f64) / GIB as f64 * 10.0).floor() / 10.0;
    Ceiling { threads: hw.cpu_cores.unwrap_or(hw.cpu_threads / 2).max(1), gpu_percent, ram_gb, has_gpu: vram > 0 }
}

/// What the app may use right now.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Limits {
    /// The mode in effect (Cool on battery may override the chosen one).
    pub mode: Mode,
    pub on_battery: bool,
    pub threads: usize,
    pub vram_bytes: u64,
    pub ram_bytes: u64,
    pub low_priority: bool,
    /// Unload an idle chat model after this many minutes (0 = never).
    pub idle_unload_mins: u64,
    pub max_ctx: u32,
    /// Pause between steps above these (°C).
    pub gpu_temp_limit: f32,
    pub cpu_temp_limit: f32,
    /// How far adaptive cooling has stepped down (0-3).
    pub heat_level: u8,
    /// Share of the model's layers kept on the graphics card (1.0 = all
    /// that fit). Lowered when the card runs hot.
    pub gpu_share: f64,
}

/// Graphics memory (all the cards models use) from which windows past 32K are offered.
pub const BIG_CARDS: u64 = 20 * GIB;

/// Always applied, whatever the settings.
const CRITICAL_GPU: f32 = 92.0;
const CRITICAL_CPU: f32 = 98.0;

pub fn limits(s: &PerfSettings, hw: &Hardware, on_battery: bool) -> Limits {
    let c = ceiling(hw);
    let vram = hw.model_vram();
    let mode = if s.cool_on_battery && on_battery { Mode::Cool } else { s.mode };
    let balanced_vram = vram.saturating_sub((vram / 10).max(512 * MIB));
    // Big graphics memory: windows up to 128K (they're still only used
    // while the whole model stays on the cards: `catalog::launch_settings`).
    let roomy_ctx = if vram >= BIG_CARDS { 131_072 } else { 32_768 };
    let balanced_ram = hw.ram_total.saturating_sub((hw.ram_total / 4).max(4 * GIB));
    let ceiling_ram = (c.ram_gb * GIB as f64) as u64;
    let (gpu_t, cpu_t) = if s.heat_guard { (87.0, 95.0) } else { (CRITICAL_GPU, CRITICAL_CPU) };
    let mut l = match mode {
        Mode::Cool => Limits {
            mode,
            on_battery,
            threads: (c.threads / 2).clamp(1, 6),
            // The whole card, as in Balanced: a smaller share only moved
            // layers into system memory and onto the processor. Fewer
            // threads, a smaller window and low priority keep it quiet.
            vram_bytes: balanced_vram,
            ram_bytes: ((hw.ram_total as f64 * 0.4) as u64).min(balanced_ram),
            low_priority: true,
            idle_unload_mins: 5,
            max_ctx: 8192,
            gpu_temp_limit: if s.heat_guard { 75.0 } else { CRITICAL_GPU },
            cpu_temp_limit: if s.heat_guard { 85.0 } else { CRITICAL_CPU },
            heat_level: 0,
            gpu_share: 1.0,
        },
        Mode::Balanced => Limits {
            mode,
            on_battery,
            threads: c.threads,
            vram_bytes: balanced_vram,
            ram_bytes: balanced_ram,
            low_priority: false,
            // A loaded model holds the card's memory: not for long unused.
            idle_unload_mins: 10,
            max_ctx: roomy_ctx,
            gpu_temp_limit: gpu_t,
            cpu_temp_limit: cpu_t,
            heat_level: 0,
            gpu_share: 1.0,
        },
        Mode::Turbo => {
            let t = s.turbo.unwrap_or(Turbo { threads: c.threads, gpu_percent: c.gpu_percent, ram_gb: c.ram_gb });
            Limits {
                mode,
                on_battery,
                threads: t.threads.clamp(1, c.threads),
                vram_bytes: (vram as f64 * t.gpu_percent.clamp(30, c.gpu_percent.max(30)) as f64 / 100.0) as u64,
                ram_bytes: ((t.ram_gb.clamp(1.0, c.ram_gb) * GIB as f64) as u64).min(ceiling_ram),
                low_priority: false,
                idle_unload_mins: 0,
                max_ctx: roomy_ctx,
                gpu_temp_limit: if s.heat_guard { 90.0 } else { CRITICAL_GPU },
                cpu_temp_limit: if s.heat_guard { 97.0 } else { CRITICAL_CPU },
                heat_level: 0,
            gpu_share: 1.0,
            }
        }
    };
    if let Some(m) = s.unload_after_mins {
        l.idle_unload_mins = m;
    }
    // Never past the ceilings, whatever was stored.
    l.vram_bytes = l.vram_bytes.min((vram as f64 * c.gpu_percent as f64 / 100.0) as u64);
    l.ram_bytes = l.ram_bytes.min(ceiling_ram);
    l.gpu_share = s.gpu_limit.min(100) as f64 / 100.0;
    // A lower GPU limit moves work to the processor: hold it to the same
    // share (a quarter of its cores at least, so the model still runs), at
    // below-normal priority, so it doesn't run hot instead. Slower, as asked.
    if s.gpu_limit < 100 {
        l.threads = ((l.threads as f64 * CPU_FLOOR.max(l.gpu_share)).round() as usize).max(1);
        l.low_priority = true;
        // And a smaller window, so that slower processor isn't asked to read
        // a 32K-token prompt (a window the person chose still applies).
        l.max_ctx = l.max_ctx.min(match s.gpu_limit {
            0 => 8192,
            1..=49 => 16_384,
            _ => 32_768,
        });
    }
    l
}

/// The least share of the processor the GPU limit leaves the model.
const CPU_FLOOR: f64 = 0.25;

pub fn settings(conn: &rusqlite::Connection) -> PerfSettings {
    db::get(conn, "perf").unwrap_or_default()
}

// ---------- adaptive cooling ----------

/// How far the app has stepped down for heat: 0 (not at all) to 3.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq)]
pub struct HeatState {
    pub gpu_level: u8,
    pub cpu_level: u8,
    pub temps: Temps,
    #[serde(skip)]
    pub changed: Option<Instant>,
}

impl HeatState {
    pub fn level(&self) -> u8 {
        self.gpu_level.max(self.cpu_level)
    }
}

#[derive(Default)]
pub struct Heat(Mutex<HeatState>);

impl Heat {
    pub fn get(&self) -> HeatState {
        *self.0.lock().unwrap()
    }

    /// Takes a new reading into account. Returns the state when a level changed.
    pub fn update(&self, t: Temps, l: &Limits) -> Option<HeatState> {
        let mut h = self.0.lock().unwrap();
        let may_ease = h.changed.map_or(true, |at| at.elapsed() >= EASE_AFTER);
        let gpu = step_level(h.gpu_level, t.gpu.map(|g| l.gpu_temp_limit - g), may_ease);
        let cpu = step_level(h.cpu_level, t.cpu.map(|c| l.cpu_temp_limit - c), may_ease);
        h.temps = t;
        if gpu == h.gpu_level && cpu == h.cpu_level {
            return None;
        }
        h.gpu_level = gpu;
        h.cpu_level = cpu;
        h.changed = Some(Instant::now());
        Some(*h)
    }

    pub fn reset(&self) -> bool {
        let mut h = self.0.lock().unwrap();
        let was = h.level() > 0;
        *h = HeatState::default();
        was
    }
}

/// Degrees below the limit at which each level starts.
const ENTER: [f32; 3] = [12.0, 7.0, 3.0];
/// A level is left only this much cooler than where it started...
const EASE_MARGIN: f32 = 4.0;
/// ...and no sooner than this after the last change, so the model isn't
/// reloaded over and over.
const EASE_AFTER: Duration = Duration::from_secs(90);

fn step_level(current: u8, headroom: Option<f32>, may_ease: bool) -> u8 {
    // No reading: nothing to go on, so don't hold the PC back.
    let Some(h) = headroom else { return 0 };
    let target = ENTER.iter().filter(|e| h <= **e).count() as u8;
    if target >= current {
        return target;
    }
    // Easing back one level at a time, once clearly cooler.
    if may_ease && h > ENTER[current as usize - 1] + EASE_MARGIN {
        current - 1
    } else {
        current
    }
}

const THREAD_SCALE: [f64; 4] = [1.0, 0.75, 0.5, 0.35];

/// The limits stepped down for heat: fewer processor threads when the
/// processor is hot. A hot card keeps its layers, and the pauses between
/// steps cool it (`step_pause`). Moving layers to the processor filled the
/// memory and left the card idle (his laptop: RAM at 100%, the card at
/// 13%), and in a laptop it only heats the shared cooler from the other side.
pub fn scaled(mut l: Limits, h: &HeatState) -> Limits {
    l.threads = ((l.threads as f64 * THREAD_SCALE[h.cpu_level.min(3) as usize]).round() as usize).max(1);
    l.heat_level = h.level();
    l
}

/// A short rest between agent steps while the PC runs hot; longer while the
/// card and the processor are both warm, since no layers move off the card then.
pub fn step_pause(h: &HeatState) -> Duration {
    let both = h.gpu_level > 0 && h.cpu_level > 0;
    match h.level() + u8::from(both) {
        0 => Duration::ZERO,
        // A warm card keeps its layers, so it rests a moment from the first level.
        1 => Duration::from_secs(1),
        2 => Duration::from_secs(2),
        _ => Duration::from_secs(5),
    }
}

// ---------- temperatures ----------

#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq)]
pub struct Temps {
    pub gpu: Option<f32>,
    pub cpu: Option<f32>,
}

fn cache() -> &'static Mutex<Option<(Instant, Temps)>> {
    static C: std::sync::OnceLock<Mutex<Option<(Instant, Temps)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

async fn gpu_temp() -> Option<f32> {
    // The driver's sensor first, as Task Manager reads it (NVIDIA cards
    // report it too): starting nvidia-smi wakes the card from idle.
    if let Some(luid) = crate::hardware::dedicated().and_then(|c| c.luid) {
        if let Some(t) = tokio::task::spawn_blocking(move || card_temp(luid)).await.ok().flatten() {
            return Some(t);
        }
    }
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"]).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let nvidia = match tokio::time::timeout(Duration::from_secs(3), cmd.output()).await {
        Ok(Ok(out)) => String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<f32>().ok()).fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.max(t)))),
        _ => None,
    };
    // Other dedicated cards (AMD, Intel Arc): the sensor Task Manager reads.
    match nvidia {
        Some(t) => Some(t),
        None => {
            let luid = crate::hardware::dedicated()?.luid?;
            tokio::task::spawn_blocking(move || card_temp(luid)).await.ok().flatten()
        }
    }
}

/// A card's temperature from its driver (what Task Manager shows), by its
/// Windows id. Not every driver reports one.
#[cfg(windows)]
fn card_temp(luid: i64) -> Option<f32> {
    use windows::Wdk::Graphics::Direct3D::*;
    use windows::Win32::Foundation::LUID;
    // SAFETY: the adapter handle is opened and closed here; the query writes
    // only into `perf`, whose size it is given.
    unsafe {
        let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: LUID { LowPart: luid as u32, HighPart: (luid >> 32) as i32 }, hAdapter: 0 };
        if D3DKMTOpenAdapterFromLuid(&mut open).is_err() {
            return None;
        }
        let mut perf = D3DKMT_ADAPTER_PERFDATA::default();
        let mut query = D3DKMT_QUERYADAPTERINFO {
            hAdapter: open.hAdapter,
            Type: KMTQAITYPE_ADAPTERPERFDATA,
            pPrivateDriverData: &mut perf as *mut _ as *mut core::ffi::c_void,
            PrivateDriverDataSize: std::mem::size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
        };
        let ok = D3DKMTQueryAdapterInfo(&mut query).is_ok();
        let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: open.hAdapter });
        // Tenths of a degree.
        let c = perf.Temperature as f32 / 10.0;
        (ok && (1.0..130.0).contains(&c)).then_some(c)
    }
}

#[cfg(not(windows))]
fn card_temp(_luid: i64) -> Option<f32> {
    None
}

#[cfg(windows)]
fn cpu_temp() -> Option<f32> {
    // ACPI thermal zones, readable without admin rights (tenths of a kelvin).
    std::thread::spawn(|| unsafe {
        let t = wmi_thermal();
        windows::Win32::System::Com::CoUninitialize();
        t
    })
    .join()
    .ok()
    .flatten()
}

#[cfg(windows)]
unsafe fn wmi_thermal() -> Option<f32> {
    use windows::core::{BSTR, VARIANT};
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Wmi::*;
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
    let svc = locator.ConnectServer(&BSTR::from("ROOT\\CIMV2"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None).ok()?;
    // Performance classes return nothing at WMI's default "identify" level.
    const RPC_C_AUTHN_WINNT: u32 = 10;
    const RPC_C_AUTHZ_NONE: u32 = 0;
    CoSetProxyBlanket(&svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE).ok()?;
    let rows = svc
        .ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT HighPrecisionTemperature FROM Win32_PerfRawData_Counters_ThermalZoneInformation"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )
        .ok()?;
    let mut best: Option<f32> = None;
    loop {
        let mut objs = [None];
        let mut n = 0u32;
        let _ = rows.Next(WBEM_INFINITE, &mut objs, &mut n);
        if n == 0 {
            break;
        }
        let Some(obj) = objs[0].take() else { break };
        let mut v = VARIANT::default();
        if obj.Get(windows::core::w!("HighPrecisionTemperature"), 0, &mut v, None, None).is_ok() {
            if let Ok(tenths_k) = u32::try_from(&v).or_else(|_| i32::try_from(&v).map(|x| x as u32)) {
                let c = tenths_k as f32 / 10.0 - 273.15;
                if (1.0..130.0).contains(&c) {
                    best = Some(best.map_or(c, |b: f32| b.max(c)));
                }
            }
        }
    }
    best
}

#[cfg(not(windows))]
fn cpu_temp() -> Option<f32> {
    None
}

/// Current temperatures, read at most every 5 seconds.
/// Temperatures without waking the graphics card: the driver's sensor only
/// (never nvidia-smi), or the last reading when it has none. For screens
/// that show temperatures while nothing is working.
pub async fn quiet_temps() -> Temps {
    if let Some((at, t)) = *cache().lock().unwrap() {
        if at.elapsed() < Duration::from_secs(5) {
            return t;
        }
    }
    let luid = crate::hardware::dedicated().and_then(|c| c.luid);
    let gpu = match luid {
        Some(l) => tokio::task::spawn_blocking(move || card_temp(l)).await.ok().flatten(),
        None => None,
    };
    let gpu = gpu.or_else(|| cache().lock().unwrap().and_then(|(_, t)| t.gpu));
    let cpu = tokio::task::spawn_blocking(cpu_temp).await.ok().flatten();
    Temps { gpu, cpu }
}

pub async fn temps() -> Temps {
    if let Some((at, t)) = *cache().lock().unwrap() {
        if at.elapsed() < Duration::from_secs(5) {
            return t;
        }
    }
    let gpu = gpu_temp().await;
    let cpu = tokio::task::spawn_blocking(cpu_temp).await.ok().flatten();
    let t = Temps { gpu, cpu };
    *cache().lock().unwrap() = Some((Instant::now(), t));
    t
}

// ---------- the graphics card right now (status bar) ----------

/// How busy and hot the graphics card is right now.
#[derive(Debug, Clone, Serialize, Default)]
pub struct GpuNow {
    pub name: Option<String>,
    /// Percent busy (every app's use).
    pub usage: Option<f32>,
    /// °C (NVIDIA cards; Windows has no general way to read others').
    pub temp: Option<f32>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    /// SulcusAI's own share: its window and the engines it started.
    pub ours: Option<f32>,
    /// SulcusAI is working (a reply, a picture, listening). While it isn't,
    /// nvidia-smi isn't run: starting it wakes the card from idle.
    pub working: bool,
}

/// SulcusAI is using the card for something now (or just was): a reply, a
/// picture or video, the microphone, a model loading.
pub fn working(state: &crate::AppState) -> bool {
    !state.generations.lock().unwrap().is_empty()
        || crate::media::any_job()
        || state.speech.in_use()
        || state.engine.try_lock().is_err()
        || state.engine_idle() < Duration::from_secs(20)
}

/// A line in the app log of the memory SulcusAI holds: this process, its
/// window (WebView2) and the engines it started, by name. Written every 10
/// minutes, so growth over hours (a leak) shows with numbers.
fn log_memory() {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let ours = our_pids();
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_memory());
    let mut by_name: std::collections::BTreeMap<String, u64> = Default::default();
    for (pid, p) in sys.processes() {
        if ours.contains(&pid.as_u32()) {
            let name = p.name().to_string_lossy().trim_end_matches(".exe").to_string();
            *by_name.entry(name).or_default() += p.memory();
        }
    }
    let total: u64 = by_name.values().sum();
    let parts: Vec<String> = by_name.iter().filter(|(_, b)| **b >= 50 << 20).map(|(n, b)| format!("{n} {:.0} MB", *b as f64 / 1048576.0)).collect();
    crate::applog::line("memory", &format!("SulcusAI and what it started hold {:.1} GB: {}", total as f64 / GIB as f64, parts.join(", ")));
}

/// SulcusAI's processes: this one and everything it started (its window's
/// WebView2 processes, the engines), refreshed every 10 seconds.
fn our_pids() -> std::collections::HashSet<u32> {
    use std::collections::{HashMap, HashSet};
    static C: Mutex<Option<(Instant, HashSet<u32>)>> = Mutex::new(None);
    if let Some((at, s)) = C.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(10) {
            return s.clone();
        }
    }
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, p) in sys.processes() {
        if let Some(parent) = p.parent() {
            children.entry(parent.as_u32()).or_default().push(pid.as_u32());
        }
    }
    let mut ours = HashSet::from([std::process::id()]);
    let mut todo = vec![std::process::id()];
    while let Some(p) = todo.pop() {
        for c in children.get(&p).into_iter().flatten() {
            if ours.insert(*c) {
                todo.push(*c);
            }
        }
    }
    *C.lock().unwrap() = Some((Instant::now(), ours.clone()));
    ours
}

/// nvidia-smi isn't there (no NVIDIA card): don't keep trying it.
static NO_NVIDIA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// NVIDIA cards: busy, temperature and memory from nvidia-smi (the card
/// with the most memory, when there are several).
pub(crate) async fn nvidia_now() -> Option<GpuNow> {
    use std::sync::atomic::Ordering;
    if NO_NVIDIA.load(Ordering::Relaxed) {
        return None;
    }
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total", "--format=csv,noheader,nounits"]).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = match tokio::time::timeout(Duration::from_secs(3), cmd.output()).await {
        Ok(Ok(o)) if o.status.success() => o,
        Ok(Err(_)) => {
            NO_NVIDIA.store(true, Ordering::Relaxed);
            return None;
        }
        _ => return None,
    };
    let mib = 1024.0 * 1024.0;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            let num = |i: usize| f.get(i).and_then(|s| s.parse::<f64>().ok());
            Some(GpuNow {
                name: Some(f.first()?.to_string()),
                usage: num(1).map(|x| x as f32),
                temp: num(2).map(|x| x as f32),
                mem_used: num(3).map(|m| (m * mib) as u64),
                mem_total: num(4).map(|m| (m * mib) as u64),
                ours: None,
                working: true,
            })
        })
        .fold(Vec::new(), |mut all: Vec<GpuNow>, g| {
            all.push(g);
            all
        })
        .into_iter()
        // The card picked in the menu, when there are two; else the biggest.
        .reduce(|a, b| {
            let picked = |g: &GpuNow| crate::hardware::main_choice().is_some_and(|m| g.name.as_deref().is_some_and(|n| crate::hardware::same_name(n, &m)));
            match (picked(&a), picked(&b)) {
                (true, _) => a,
                (_, true) => b,
                _ if b.mem_total.unwrap_or(0) > a.mem_total.unwrap_or(0) => b,
                _ => a,
            }
        })
}

/// A card on Windows: how busy it is, the way Task Manager counts it (its
/// busiest kind of engine), for every app and for `ours` alone. `card` is
/// its Windows id; without one, the busiest card. The query stays open,
/// since a rate needs two readings. Reading these counters doesn't wake the
/// card the way starting nvidia-smi does.
#[cfg(windows)]
fn windows_gpu_usage(card: Option<i64>, ours: &std::collections::HashSet<u32>) -> Option<(f32, f32)> {
    use std::collections::HashMap;
    use windows::Win32::System::Performance::*;
    static QUERY: Mutex<Option<(isize, isize)>> = Mutex::new(None);
    let mut q = QUERY.lock().unwrap();
    // SAFETY: PDH calls on handles this function owns; the item array is read
    // only within the buffer PDH filled, which is 8-byte aligned.
    unsafe {
        if q.is_none() {
            let (mut query, mut counter) = (0isize, 0isize);
            if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            if PdhAddEnglishCounterW(query, windows::core::w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            PdhCollectQueryData(query);
            *q = Some((query, counter));
            std::thread::sleep(Duration::from_millis(250));
        }
        let (query, counter) = (*q)?;
        if PdhCollectQueryData(query) != 0 {
            return None;
        }
        let (mut size, mut count) = (0u32, 0u32);
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None) != PDH_MORE_DATA || size == 0 {
            return None;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
            return None;
        }
        // Instances look like "pid_1234_luid_0x00000000_0x0001A2B8_phys_0_eng_3_engtype_3D".
        let wanted = card.map(|l| format!("0x{:08x}_0x{:08x}", (l >> 32) as u32, l as u32));
        let mut sums: HashMap<String, f64> = HashMap::new();
        let mut mine: HashMap<String, f64> = HashMap::new();
        for i in 0..count as usize {
            let item = &*items.add(i);
            if item.FmtValue.CStatus != 0 {
                continue;
            }
            let name = item.szName.to_string().unwrap_or_default().to_lowercase();
            let card = name.split("luid_").nth(1).and_then(|r| r.split("_phys").next()).unwrap_or("");
            if wanted.as_deref().is_some_and(|w| w != card) {
                continue;
            }
            let kind = name.rsplit("engtype_").next().unwrap_or("");
            let value = item.FmtValue.Anonymous.doubleValue;
            let key = format!("{card}/{kind}");
            *sums.entry(key.clone()).or_default() += value;
            let pid = name.strip_prefix("pid_").and_then(|r| r.split('_').next()).and_then(|p| p.parse::<u32>().ok());
            if pid.is_some_and(|p| ours.contains(&p)) {
                *mine.entry(key).or_default() += value;
            }
        }
        let busiest = |m: HashMap<String, f64>| m.into_values().reduce(f64::max).map_or(0.0, |v| v.clamp(0.0, 100.0) as f32);
        if sums.is_empty() {
            return None;
        }
        Some((busiest(sums), busiest(mine)))
    }
}

#[cfg(not(windows))]
fn windows_gpu_usage(_card: Option<i64>, _ours: &std::collections::HashSet<u32>) -> Option<(f32, f32)> {
    None
}

/// The graphics card's own memory in use right now, by every app (Windows'
/// "GPU Adapter Memory" counters: any vendor, and it doesn't wake the card).
#[cfg(windows)]
pub fn card_memory_used(card: Option<i64>) -> Option<u64> {
    use windows::Win32::System::Performance::*;
    // SAFETY: as in `windows_gpu_usage`: PDH handles opened and closed here,
    // items read only within the buffer PDH filled.
    unsafe {
        let (mut query, mut counter) = (0isize, 0isize);
        if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut query) != 0 {
            return None;
        }
        let close = |q: isize| {
            PdhCloseQuery(q);
        };
        if PdhAddEnglishCounterW(query, windows::core::w!("\\GPU Adapter Memory(*)\\Dedicated Usage"), 0, &mut counter) != 0 || PdhCollectQueryData(query) != 0 {
            close(query);
            return None;
        }
        let (mut size, mut count) = (0u32, 0u32);
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut size, &mut count, None) != PDH_MORE_DATA || size == 0 {
            close(query);
            return None;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut size, &mut count, Some(items)) != 0 {
            close(query);
            return None;
        }
        // Instances look like "luid_0x00000000_0x0001A2B8_phys_0".
        let wanted = card.map(|l| format!("0x{:08x}_0x{:08x}", (l >> 32) as u32, l as u32));
        let mut used: Option<u64> = None;
        for i in 0..count as usize {
            let item = &*items.add(i);
            if item.FmtValue.CStatus != 0 {
                continue;
            }
            let name = item.szName.to_string().unwrap_or_default().to_lowercase();
            let this = name.split("luid_").nth(1).and_then(|r| r.split("_phys").next()).unwrap_or("");
            if wanted.as_deref().is_some_and(|w| w != this) {
                continue;
            }
            let v = item.FmtValue.Anonymous.largeValue.max(0) as u64;
            // Without a card named, the busiest one.
            used = Some(if wanted.is_some() { used.unwrap_or(0) + v } else { used.map_or(v, |u| u.max(v)) });
        }
        close(query);
        used
    }
}

#[cfg(not(windows))]
pub fn card_memory_used(_card: Option<i64>) -> Option<u64> {
    None
}

/// One process's graphics memory: on the card itself, and in system memory
/// the card uses ("shared"). Windows moves what doesn't fit on the card to
/// shared memory without telling the app: a model that looks loaded is then
/// slow in long chats (2.1 GB of a 14B spilled at 16K with the normal cache).
#[cfg(windows)]
pub fn process_card_memory(pid: u32) -> Option<(u64, u64)> {
    let sum = |path: &str| -> Option<u64> {
        use windows::Win32::System::Performance::*;
        // SAFETY: as in `card_memory_used`.
        unsafe {
            let (mut query, mut counter) = (0isize, 0isize);
            if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let mut out = None;
            if PdhAddEnglishCounterW(query, windows::core::PCWSTR(wide.as_ptr()), 0, &mut counter) == 0 && PdhCollectQueryData(query) == 0 {
                let (mut size, mut count) = (0u32, 0u32);
                if PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut size, &mut count, None) == PDH_MORE_DATA && size > 0 {
                    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
                    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut size, &mut count, Some(items)) == 0 {
                        let mut total = 0u64;
                        for i in 0..count as usize {
                            let item = &*items.add(i);
                            if item.FmtValue.CStatus == 0 {
                                total += item.FmtValue.Anonymous.largeValue.max(0) as u64;
                            }
                        }
                        out = Some(total);
                    }
                }
            }
            PdhCloseQuery(query);
            out
        }
    };
    let dedicated = sum(&format!("\\GPU Process Memory(pid_{pid}_*)\\Dedicated Usage"))?;
    let shared = sum(&format!("\\GPU Process Memory(pid_{pid}_*)\\Shared Usage")).unwrap_or(0);
    Some((dedicated, shared))
}

#[cfg(not(windows))]
pub fn process_card_memory(_pid: u32) -> Option<(u64, u64)> {
    None
}

fn gpu_cache() -> &'static Mutex<Option<(Instant, GpuNow)>> {
    static C: std::sync::OnceLock<Mutex<Option<(Instant, GpuNow)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// The graphics card right now, read at most every 2 seconds.
#[tauri::command]
pub async fn gpu_now(state: AppStateRef<'_>) -> Result<GpuNow, String> {
    Ok(gpu_reading(&state).await)
}

/// The processor's use (percent of all cores) since the last reading.
pub fn cpu_usage() -> f32 {
    static SYS: std::sync::OnceLock<Mutex<sysinfo::System>> = std::sync::OnceLock::new();
    let sys = SYS.get_or_init(|| {
        let mut s = sysinfo::System::new();
        s.refresh_cpu_usage();
        Mutex::new(s)
    });
    let mut s = sys.lock().unwrap();
    s.refresh_cpu_usage();
    s.global_cpu_usage()
}

/// The dedicated card's use and temperature now (cached for 2 seconds).
pub async fn gpu_reading(state: &crate::AppState) -> GpuNow {
    if let Some((at, g)) = gpu_cache().lock().unwrap().clone() {
        if at.elapsed() < Duration::from_secs(2) {
            return g;
        }
    }
    // The dedicated card (the one models run on), never the integrated one.
    let best = state.hardware.read().unwrap().best_gpu().cloned();
    let working = working(state);
    let luid = best.as_ref().and_then(|c| c.luid);
    let ours = our_pids();
    let counted = tokio::task::spawn_blocking(move || windows_gpu_usage(luid, &ours)).await.ok().flatten();
    // nvidia-smi (temperature, memory) only while SulcusAI is working: each
    // start of it wakes the card from idle.
    let nvidia = match &best {
        Some(card) if card.vendor != "NVIDIA" => None,
        _ if working => nvidia_now().await,
        _ => None,
    };
    let mut g = match nvidia {
        Some(g) => g,
        None => {
            let Some(card) = best else { return GpuNow::default() };
            GpuNow {
                name: Some(card.name.clone()),
                usage: counted.map(|c| c.0),
                // From the driver, as Task Manager reads it (NVIDIA cards too),
                // which doesn't wake the card.
                temp: match luid {
                    Some(l) => tokio::task::spawn_blocking(move || card_temp(l)).await.ok().flatten(),
                    None => None,
                },
                mem_used: None,
                mem_total: Some(card.vram_bytes),
                ours: None,
                working,
            }
        }
    };
    g.ours = counted.map(|c| c.1);
    g.working = working;
    *gpu_cache().lock().unwrap() = Some((Instant::now(), g.clone()));
    g
}

fn too_hot(t: Temps, l: &Limits, margin: f32) -> Option<String> {
    if let Some(g) = t.gpu.filter(|g| *g >= l.gpu_temp_limit - margin) {
        return Some(format!("graphics card {g:.0}°C"));
    }
    if let Some(c) = t.cpu.filter(|c| *c >= l.cpu_temp_limit - margin) {
        return Some(format!("processor {c:.0}°C"));
    }
    None
}

/// Waits (up to two minutes) while the PC is over its temperature limit,
/// until it is 5 °C below. `waiting` is told what is hot, once.
pub async fn cool_down(l: &Limits, waiting: impl FnOnce(&str)) {
    let Some(what) = too_hot(temps().await, l, 0.0) else { return };
    waiting(&what);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(120) {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if too_hot(temps().await, l, 5.0).is_none() {
            return;
        }
    }
}

// ---------- watching ----------

const WATCH_EVERY: Duration = Duration::from_secs(10);

/// Every 10 seconds while a model is loaded: reads the temperatures, steps
/// the limits up or down (adaptive cooling), and unloads an idle chat model.
pub fn start_monitor(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    tauri::async_runtime::spawn(async move {
        let state = app.state::<std::sync::Arc<crate::AppState>>().inner().clone();
        let mut power = crate::hardware::on_battery();
        let mut memory_noted = Instant::now();
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            if memory_noted.elapsed() >= Duration::from_secs(600) {
                memory_noted = Instant::now();
                let _ = tokio::task::spawn_blocking(log_memory).await;
            }
            // Plugged in or unplugged: some laptops turn their graphics card
            // on or off with the power source, so the PC is checked again.
            let now = crate::hardware::on_battery();
            if now != power {
                power = now;
                let s = state.clone();
                let why = if now == Some(true) { "unplugging" } else { "plugging in" };
                tokio::task::spawn_blocking(move || s.recheck_hardware(why)).await.ok();
            }
            let settings = settings(&state.db.lock().unwrap());
            let base = {
                let hw = state.hardware.read().unwrap();
                limits(&settings, &hw, crate::hardware::on_battery() == Some(true))
            };
            // While loading, the engine lock is held: count that as loaded.
            let loaded = match state.engine.try_lock() {
                Ok(mut e) => e.status().model_id.is_some(),
                Err(_) => true,
            };
            let busy = !state.generations.lock().unwrap().is_empty();

            // Temperatures only while it works: reading the card's (nvidia-smi)
            // wakes it, and a loaded model at rest doesn't heat anything.
            if settings.adaptive && working(&state) {
                if let Some(h) = state.heat.update(temps().await, &base) {
                    let what = if h.level() == 0 { "back to full speed".to_string() } else { format!("stepped down to level {} of 3", h.level()) };
                    db::log_action(&state.db.lock().unwrap(), "performance", &format!("Adaptive cooling: {what}"));
                    app.emit("perf:heat", h).ok();
                }
            } else if !settings.adaptive && state.heat.reset() {
                app.emit("perf:heat", HeatState::default()).ok();
            }

            let idle = state.engine_idle();
            let after = if settings.unload_right_away { Some(Duration::from_secs(15)) } else { (base.idle_unload_mins > 0).then(|| Duration::from_secs(base.idle_unload_mins * 60)) };
            if loaded && !busy && after.is_some_and(|a| idle >= a) {
                if let Ok(mut e) = state.engine.try_lock() {
                    if e.status().model_id.is_some() {
                        e.stop().await;
                        let why = if settings.unload_right_away { "right after its reply".to_string() } else { format!("after {} idle minutes", base.idle_unload_mins) };
                        db::log_action(&state.db.lock().unwrap(), "performance", &format!("Unloaded the chat model {why}"));
                        app.emit("engine:unloaded", ()).ok();
                    }
                }
            }
        }
    });
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct PerfView {
    settings: PerfSettings,
    ceiling: Ceiling,
    /// What the mode allows, before adaptive cooling.
    base: Limits,
    /// What the app may use right now.
    limits: Limits,
    heat: HeatState,
    temps: Temps,
    has_battery: bool,
    cores: usize,
    ram_total_gb: f64,
    vram_gb: f64,
}

/// The loaded chat model now: working, or idle for how long, and when it
/// unloads by itself (None: it stays loaded, as Turbo or the setting says).
#[derive(Serialize)]
pub struct ModelIdle {
    busy: bool,
    idle_secs: u64,
    unloads_in: Option<u64>,
}

#[tauri::command]
pub fn model_idle(state: AppStateRef) -> ModelIdle {
    let s = settings(&state.db.lock().unwrap());
    let base = limits(&s, &state.hardware.read().unwrap(), crate::hardware::on_battery() == Some(true));
    let idle = state.engine_idle().as_secs();
    let after = if s.unload_right_away { Some(15) } else { (base.idle_unload_mins > 0).then_some(base.idle_unload_mins * 60) };
    let busy = !state.generations.lock().unwrap().is_empty() || crate::media::any_job();
    ModelIdle { busy, idle_secs: if busy { 0 } else { idle }, unloads_in: after.map(|a| a.saturating_sub(idle) + WATCH_EVERY.as_secs()) }
}

#[tauri::command]
pub async fn perf_view(state: AppStateRef<'_>) -> Result<PerfView, String> {
    let hw = state.hardware.read().unwrap().clone();
    let settings = settings(&state.db.lock().unwrap());
    let base = limits(&settings, &hw, crate::hardware::on_battery() == Some(true));
    Ok(PerfView {
        ceiling: ceiling(&hw),
        base,
        limits: state.limits(),
        heat: state.heat.get(),
        // Read every 30 s while the window shows: it mustn't wake the card when nothing's working.
        temps: if working(&state) { temps().await } else { quiet_temps().await },
        has_battery: crate::hardware::has_battery(),
        cores: hw.cpu_cores.unwrap_or(hw.cpu_threads / 2),
        ram_total_gb: hw.ram_total as f64 / GIB as f64,
        vram_gb: hw.best_gpu().map_or(0.0, |g| g.vram_bytes as f64 / GIB as f64),
        settings,
    })
}

#[tauri::command]
pub async fn set_perf(state: AppStateRef<'_>, settings: PerfSettings) -> Result<Limits, String> {
    let hw = state.hardware.read().unwrap().clone();
    let c = ceiling(&hw);
    let mut s = settings;
    // Store Turbo values already clamped to what this PC can give.
    if let Some(t) = s.turbo.as_mut() {
        t.threads = t.threads.clamp(1, c.threads);
        t.gpu_percent = t.gpu_percent.clamp(30, c.gpu_percent.max(30));
        t.ram_gb = t.ram_gb.clamp(1.0, c.ram_gb);
    }
    {
        let conn = state.db.lock().unwrap();
        // The graphics card limit is set at the top of a chat, not here.
        s.gpu_limit = self::settings(&conn).gpu_limit;
        db::set(&conn, "perf", &s)?;
        let label = match s.mode {
            Mode::Cool => "Cool & quiet",
            Mode::Balanced => "Balanced",
            Mode::Turbo => "Turbo",
        };
        db::log_action(&conn, "performance", &format!("Performance set to {label}"));
    }
    if !s.adaptive && state.heat.reset() {
        db::log_action(&state.db.lock().unwrap(), "performance", "Adaptive cooling turned off; back to full limits");
    }
    // The running model restarts with the new limits on its next use.
    Ok(state.limits())
}

#[derive(Serialize)]
pub struct GpuLimit {
    percent: u8,
    has_gpu: bool,
}

fn gpu_limit_of(state: &crate::AppState) -> GpuLimit {
    GpuLimit {
        percent: settings(&state.db.lock().unwrap()).gpu_limit,
        has_gpu: state.hardware.read().unwrap().best_gpu().is_some_and(|g| g.vram_bytes > 0),
    }
}

/// How much of the model may run on the graphics card (top of a chat).
#[tauri::command(async)]
pub fn gpu_limit(state: AppStateRef) -> GpuLimit {
    gpu_limit_of(&state)
}

/// Sets it; the model reloads with it on its next reply.
#[tauri::command(async)]
pub fn set_gpu_limit(app: tauri::AppHandle, state: AppStateRef, percent: u8) -> Result<GpuLimit, String> {
    use tauri::Emitter;
    let percent = percent.min(100);
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.gpu_limit = percent;
        db::set(&conn, "perf", &s)?;
        db::log_action(&conn, "performance", &format!("Graphics card limit set to {percent}%"));
    }
    app.emit("gpu:limit", serde_json::json!({ "percent": percent })).ok();
    Ok(gpu_limit_of(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The memory note names this process and adds up what it holds.
    #[test]
    fn memory_is_noted_by_process() {
        log_memory();
    }
    use crate::hardware::Gpu;

    fn laptop() -> Hardware {
        Hardware {
            cpu_cores: Some(8),
            cpu_threads: 16,
            ram_total: 16 * GIB,
            gpus: vec![Gpu { name: "GPU".into(), vendor: "NVIDIA".into(), vram_bytes: 8 * GIB, integrated: false, luid: None, dxgi: None }],
            ..Default::default()
        }
    }

    #[test]
    fn ceilings_leave_room_for_windows() {
        let c = ceiling(&laptop());
        assert_eq!(c.threads, 8);
        assert_eq!(c.gpu_percent, 95);
        assert_eq!(c.ram_gb, 13.0, "16 GB minus 3 GB kept for Windows");
    }

    #[test]
    fn turbo_can_never_exceed_the_ceiling() {
        let hw = laptop();
        let s = PerfSettings { mode: Mode::Turbo, turbo: Some(Turbo { threads: 64, gpu_percent: 100, ram_gb: 64.0 }), ..Default::default() };
        let l = limits(&s, &hw, false);
        assert_eq!(l.threads, 8);
        assert!(l.vram_bytes <= (8.0 * 0.95 * GIB as f64) as u64);
        assert_eq!(l.ram_bytes, 13 * GIB);
    }

    #[test]
    fn cool_uses_less_and_battery_switches_to_it() {
        let hw = laptop();
        let balanced = limits(&PerfSettings::default(), &hw, false);
        let on_battery = limits(&PerfSettings::default(), &hw, true);
        assert_eq!(on_battery.mode, Mode::Cool);
        assert!(on_battery.threads < balanced.threads && on_battery.vram_bytes == balanced.vram_bytes && on_battery.ram_bytes < balanced.ram_bytes);
        assert!(on_battery.low_priority && on_battery.max_ctx == 8192);
        let s = PerfSettings { cool_on_battery: false, ..Default::default() };
        assert_eq!(limits(&s, &hw, true).mode, Mode::Balanced);
    }

    #[test]
    fn adaptive_steps_down_when_hot_and_back_up_slowly() {
        let l = limits(&PerfSettings::default(), &laptop(), false); // GPU limit 87
        assert_eq!(step_level(0, Some(20.0), true), 0);
        assert_eq!(step_level(0, Some(10.0), true), 1, "77 C: within 12 of the limit");
        assert_eq!(step_level(1, Some(2.0), false), 3, "steps down at once, even soon after a change");
        assert_eq!(step_level(3, Some(6.0), true), 3, "not yet 4 C cooler than where level 3 starts");
        assert_eq!(step_level(3, Some(8.0), true), 2, "eases one level at a time");
        assert_eq!(step_level(2, Some(30.0), false), 2, "waits before easing");
        assert_eq!(step_level(2, None, false), 0, "no reading: no limit");

        let h = HeatState { gpu_level: 2, cpu_level: 0, ..Default::default() };
        let s = scaled(l, &h);
        assert_eq!(s.threads, l.threads, "a hot graphics card doesn't cut processor threads");
        assert!(s.gpu_share == 1.0 && s.heat_level == 2, "a hot card keeps its layers; pauses cool it");
        assert_eq!(step_pause(&h), Duration::from_secs(2));

        let heat = Heat::default();
        assert!(heat.update(Temps { gpu: Some(82.0), cpu: Some(60.0) }, &l).is_some());
        assert_eq!(heat.get().gpu_level, 2);
        assert!(heat.update(Temps { gpu: Some(82.0), cpu: Some(60.0) }, &l).is_none(), "no change, no event");
    }

    /// Heat never moves layers off the card (into system memory, onto the
    /// processor): the processor steps down and the pauses grow instead.
    #[test]
    fn heat_never_moves_layers_off_the_card() {
        let l = limits(&PerfSettings::default(), &laptop(), false);
        let card_hot = HeatState { gpu_level: 3, cpu_level: 0, ..Default::default() };
        let both_hot = HeatState { gpu_level: 2, cpu_level: 1, ..Default::default() };
        assert_eq!(scaled(l, &card_hot).gpu_share, 1.0);
        assert_eq!(scaled(l, &both_hot).gpu_share, 1.0);
        assert!(scaled(l, &both_hot).threads < l.threads, "the processor still steps down");
        assert!(step_pause(&card_hot) > Duration::ZERO, "a hot card gets pauses");
        assert!(step_pause(&both_hot) > step_pause(&HeatState { gpu_level: 2, cpu_level: 0, ..Default::default() }));
        // A GPU limit the person set still holds.
        let half = limits(&PerfSettings { gpu_limit: 50, ..Default::default() }, &laptop(), false);
        assert_eq!(scaled(half, &card_hot).gpu_share, 0.5);
        // Cool & quiet on battery uses the whole card too.
        assert_eq!(limits(&PerfSettings::default(), &laptop(), true).vram_bytes, l.vram_bytes);
    }

    /// Reads this PC's sensors: `cargo test --lib reads_temperatures -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn reads_temperatures() {
        let t = temps().await;
        println!("{t:?}");
        assert!(t.gpu.is_some() || t.cpu.is_some(), "no sensor readable");
    }

    #[test]
    fn an_idle_model_frees_the_card_when_asked() {
        assert_eq!(limits(&PerfSettings::default(), &laptop(), false).idle_unload_mins, 10, "Balanced: after 10 minutes");
        assert_eq!(limits(&PerfSettings { unload_after_mins: Some(2), ..Default::default() }, &laptop(), false).idle_unload_mins, 2);
        assert_eq!(limits(&PerfSettings { mode: Mode::Cool, unload_after_mins: Some(0), ..Default::default() }, &laptop(), false).idle_unload_mins, 0, "never");
    }

    #[test]
    fn the_critical_limit_stays_on_without_the_heat_guard() {
        let s = PerfSettings { heat_guard: false, ..Default::default() };
        let l = limits(&s, &laptop(), false);
        assert_eq!(l.gpu_temp_limit, CRITICAL_GPU);
        assert!(too_hot(Temps { gpu: Some(95.0), cpu: None }, &l, 0.0).is_some());
        assert!(too_hot(Temps { gpu: Some(80.0), cpu: Some(70.0) }, &l, 0.0).is_none());
    }

    /// The limit set at the top of a chat: that share of the layers on the
    /// card, and a hot card never puts more back.
    #[test]
    fn the_gpu_limit_keeps_part_of_the_model_off_the_card() {
        let full = limits(&PerfSettings::default(), &laptop(), false);
        assert_eq!(full.gpu_share, 1.0);
        let half = limits(&PerfSettings { gpu_limit: 50, ..Default::default() }, &laptop(), false);
        assert_eq!(half.gpu_share, 0.5);
        let warm = HeatState { gpu_level: 1, cpu_level: 0, temps: Temps::default(), changed: None };
        assert_eq!(scaled(half, &warm).gpu_share, 0.5);
        assert_eq!(scaled(full, &warm).gpu_share, 1.0, "heat doesn't lower it");
        // The processor is held back to match, so it doesn't run hot instead.
        assert!(!full.low_priority);
        assert_eq!(half.threads, ((full.threads as f64) * 0.5).round() as usize);
        assert!(half.low_priority);
        let off = limits(&PerfSettings { gpu_limit: 0, ..Default::default() }, &laptop(), false);
        assert_eq!(off.threads, ((full.threads as f64) * 0.25).round().max(1.0) as usize, "a quarter of the cores at least");
        assert!(off.max_ctx <= 8192 && half.max_ctx <= 32_768 && limits(&PerfSettings { gpu_limit: 25, ..Default::default() }, &laptop(), false).max_ctx <= 16_384, "a smaller window for a slower processor");
        assert!(scaled(half, &HeatState { gpu_level: 0, cpu_level: 2, temps: Temps::default(), changed: None }).threads <= half.threads, "a hot processor only lowers it");
        let m = crate::catalog::Catalog::bundled().model("qwen3-8b").cloned().unwrap();
        let quant = m.variants[0].quant.clone();
        let b = crate::catalog::Budget { vram: 8 * GIB, vram_card: 8 * GIB, cards: 1, ram: 12 * GIB, disk: 100 * GIB, gpu_bw_gbs: 400.0, gpu_bw_card: 400.0, cpu_bw_gbs: 50.0 };
        let (_, all) = crate::catalog::launch_within(&m, &quant, &b, &full);
        let (_, some) = crate::catalog::launch_within(&m, &quant, &b, &half);
        let (_, none) = crate::catalog::launch_within(&m, &quant, &b, &off);
        assert!(all > 0 && some == all / 2 && none == 0, "{all} {some} {none}");
    }
}

#[cfg(test)]
mod gpu_tests {
    /// Reads this PC's graphics card: `cargo test --lib reads_the_gpu -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn reads_the_gpu() {
        eprintln!("nvidia-smi: {:?}", super::nvidia_now().await);
        crate::hardware::detect(&std::env::temp_dir());
        let card = crate::hardware::dedicated();
        eprintln!("dedicated card: {:?}", card.as_ref().map(|c| &c.name));
        let luid = card.and_then(|c| c.luid);
        let ours = super::our_pids();
        eprintln!("our processes: {}", ours.len());
        let again = ours.clone();
        eprintln!("Windows counters, that card (all, ours): {:?}", tokio::task::spawn_blocking(move || super::windows_gpu_usage(luid, &ours)).await.unwrap());
        eprintln!("again: {:?}", tokio::task::spawn_blocking(move || super::windows_gpu_usage(luid, &again)).await.unwrap());
        eprintln!("its temperature: {:?}", luid.and_then(super::card_temp));
    }
}

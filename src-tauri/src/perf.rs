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
}

impl Default for PerfSettings {
    fn default() -> Self {
        PerfSettings { mode: Mode::Balanced, turbo: None, cool_on_battery: true, heat_guard: true, adaptive: true, gpu_limit: 100 }
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
    let vram = hw.best_gpu().map_or(0, |g| g.vram_bytes);
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

/// Always applied, whatever the settings.
const CRITICAL_GPU: f32 = 92.0;
const CRITICAL_CPU: f32 = 98.0;

pub fn limits(s: &PerfSettings, hw: &Hardware, on_battery: bool) -> Limits {
    let c = ceiling(hw);
    let vram = hw.best_gpu().map_or(0, |g| g.vram_bytes);
    let mode = if s.cool_on_battery && on_battery { Mode::Cool } else { s.mode };
    let balanced_vram = vram.saturating_sub((vram / 10).max(512 * MIB));
    let balanced_ram = hw.ram_total.saturating_sub((hw.ram_total / 4).max(4 * GIB));
    let ceiling_ram = (c.ram_gb * GIB as f64) as u64;
    let (gpu_t, cpu_t) = if s.heat_guard { (87.0, 95.0) } else { (CRITICAL_GPU, CRITICAL_CPU) };
    let mut l = match mode {
        Mode::Cool => Limits {
            mode,
            on_battery,
            threads: (c.threads / 2).clamp(1, 6),
            vram_bytes: (vram as f64 * 0.7) as u64,
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
            idle_unload_mins: 30,
            max_ctx: 32_768,
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
                max_ctx: 32_768,
                gpu_temp_limit: if s.heat_guard { 90.0 } else { CRITICAL_GPU },
                cpu_temp_limit: if s.heat_guard { 97.0 } else { CRITICAL_CPU },
                heat_level: 0,
            gpu_share: 1.0,
            }
        }
    };
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
const GPU_SCALE: [f64; 4] = [1.0, 0.85, 0.7, 0.55];

/// The limits stepped down for heat.
pub fn scaled(mut l: Limits, h: &HeatState) -> Limits {
    l.threads = ((l.threads as f64 * THREAD_SCALE[h.cpu_level.min(3) as usize]).round() as usize).max(1);
    l.gpu_share = l.gpu_share.min(GPU_SCALE[h.gpu_level.min(3) as usize]);
    l.heat_level = h.level();
    l
}

/// A short rest between agent steps while the PC runs hot.
pub fn step_pause(h: &HeatState) -> Duration {
    match h.level() {
        0 | 1 => Duration::ZERO,
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
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"]).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(3), cmd.output()).await.ok()?.ok()?;
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<f32>().ok()).fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.max(t))))
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
    /// Percent busy.
    pub usage: Option<f32>,
    /// °C (NVIDIA cards; Windows has no general way to read others').
    pub temp: Option<f32>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
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
            })
        })
        .max_by_key(|g| g.mem_total.unwrap_or(0))
}

/// Any card on Windows: how busy it is, the way Task Manager counts it
/// (the busiest kind of engine on the busiest card). The query stays open,
/// since a rate needs two readings.
#[cfg(windows)]
fn windows_gpu_usage() -> Option<f32> {
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
        // Instances look like "pid_1234_luid_0x0_0x1A2B_phys_0_eng_3_engtype_3D".
        let mut sums: HashMap<String, f64> = HashMap::new();
        for i in 0..count as usize {
            let item = &*items.add(i);
            if item.FmtValue.CStatus != 0 {
                continue;
            }
            let name = item.szName.to_string().unwrap_or_default();
            let card = name.split("luid_").nth(1).and_then(|r| r.split("_phys").next()).unwrap_or("");
            let kind = name.rsplit("engtype_").next().unwrap_or("");
            *sums.entry(format!("{card}/{kind}")).or_default() += item.FmtValue.Anonymous.doubleValue;
        }
        sums.into_values().reduce(f64::max).map(|v| v.clamp(0.0, 100.0) as f32)
    }
}

#[cfg(not(windows))]
fn windows_gpu_usage() -> Option<f32> {
    None
}

fn gpu_cache() -> &'static Mutex<Option<(Instant, GpuNow)>> {
    static C: std::sync::OnceLock<Mutex<Option<(Instant, GpuNow)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// The graphics card right now, read at most every 2 seconds.
#[tauri::command]
pub async fn gpu_now(state: AppStateRef<'_>) -> Result<GpuNow, String> {
    if let Some((at, g)) = gpu_cache().lock().unwrap().clone() {
        if at.elapsed() < Duration::from_secs(2) {
            return Ok(g);
        }
    }
    let g = match nvidia_now().await {
        Some(g) => g,
        None => {
            let best = state.hardware.read().unwrap().best_gpu().cloned();
            let Some(card) = best else { return Ok(GpuNow::default()) };
            GpuNow {
                name: Some(card.name.clone()),
                usage: tokio::task::spawn_blocking(windows_gpu_usage).await.ok().flatten(),
                temp: None,
                mem_used: None,
                mem_total: Some(card.vram_bytes),
            }
        }
    };
    *gpu_cache().lock().unwrap() = Some((Instant::now(), g.clone()));
    Ok(g)
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
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
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
            let listening = state.speech.in_use();
            let busy = !state.generations.lock().unwrap().is_empty();

            if settings.adaptive && (loaded || listening) {
                if let Some(h) = state.heat.update(temps().await, &base) {
                    let what = if h.level() == 0 { "back to full speed".to_string() } else { format!("stepped down to level {} of 3", h.level()) };
                    db::log_action(&state.db.lock().unwrap(), "performance", &format!("Adaptive cooling: {what}"));
                    app.emit("perf:heat", h).ok();
                }
            } else if !settings.adaptive && state.heat.reset() {
                app.emit("perf:heat", HeatState::default()).ok();
            }

            let idle = state.engine_idle();
            if loaded && !busy && base.idle_unload_mins > 0 && idle >= Duration::from_secs(base.idle_unload_mins * 60) {
                if let Ok(mut e) = state.engine.try_lock() {
                    if e.status().model_id.is_some() {
                        e.stop().await;
                        db::log_action(&state.db.lock().unwrap(), "performance", &format!("Unloaded the chat model after {} idle minutes", base.idle_unload_mins));
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
        temps: temps().await,
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
    use crate::hardware::Gpu;

    fn laptop() -> Hardware {
        Hardware {
            cpu_cores: Some(8),
            cpu_threads: 16,
            ram_total: 16 * GIB,
            gpus: vec![Gpu { name: "GPU".into(), vendor: "NVIDIA".into(), vram_bytes: 8 * GIB, integrated: false }],
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
        assert!(on_battery.threads < balanced.threads && on_battery.vram_bytes < balanced.vram_bytes && on_battery.ram_bytes < balanced.ram_bytes);
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
        assert!(s.gpu_share < 1.0 && s.heat_level == 2);
        assert_eq!(step_pause(&h), Duration::from_secs(2));

        let heat = Heat::default();
        assert!(heat.update(Temps { gpu: Some(82.0), cpu: Some(60.0) }, &l).is_some());
        assert_eq!(heat.get().gpu_level, 2);
        assert!(heat.update(Temps { gpu: Some(82.0), cpu: Some(60.0) }, &l).is_none(), "no change, no event");
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
        assert_eq!(scaled(half, &warm).gpu_share, 0.5, "the lower of the two");
        assert_eq!(scaled(full, &warm).gpu_share, GPU_SCALE[1]);
        // The processor is held back to match, so it doesn't run hot instead.
        assert!(!full.low_priority);
        assert_eq!(half.threads, ((full.threads as f64) * 0.5).round() as usize);
        assert!(half.low_priority);
        let off = limits(&PerfSettings { gpu_limit: 0, ..Default::default() }, &laptop(), false);
        assert_eq!(off.threads, ((full.threads as f64) * 0.25).round().max(1.0) as usize, "a quarter of the cores at least");
        assert!(scaled(half, &HeatState { gpu_level: 0, cpu_level: 2, temps: Temps::default(), changed: None }).threads <= half.threads, "a hot processor only lowers it");
        let m = crate::catalog::Catalog::bundled().model("qwen3-8b").cloned().unwrap();
        let quant = m.variants[0].quant.clone();
        let b = crate::catalog::Budget { vram: 8 * GIB, ram: 12 * GIB, disk: 100 * GIB, gpu_bw_gbs: 400.0, cpu_bw_gbs: 50.0 };
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
        eprintln!("Windows counters: {:?}", tokio::task::spawn_blocking(super::windows_gpu_usage).await.unwrap());
        eprintln!("again: {:?}", tokio::task::spawn_blocking(super::windows_gpu_usage).await.unwrap());
    }
}

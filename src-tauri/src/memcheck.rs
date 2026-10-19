// SPDX-License-Identifier: AGPL-3.0-only
//! Before a model loads: is there room for it now? Free system memory and
//! graphics memory are read at that moment (a game, a browser full of tabs
//! or a VM may be using most of it), counting back what the model it
//! replaces gives up. When the model would leave too little, the person is
//! warned, with the apps using the most memory, and chooses: load it anyway,
//! or not.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;
use tokio::sync::oneshot;

use crate::catalog::{kv_cache_bytes, kv_cache_bytes_q8, ModelSpec, GIB, RUNTIME_OVERHEAD};
use crate::AppState;

const MIB: u64 = 1024 * 1024;
/// Left free for Windows and the apps already open.
const KEEP_RAM: u64 = 3 * GIB / 2;
/// A little past the card's free memory spills into shared memory (a bit
/// slower, and the planner already fills most of the card); more than this,
/// with a game holding the card, stutters.
const VRAM_SPILL: u64 = 512 * MIB;
/// How long the question waits before the answer counts as no.
const WAIT: Duration = Duration::from_secs(180);

/// What a model takes in system memory and on the graphics card.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Need {
    pub ram: u64,
    pub vram: u64,
}

/// A model at this context with `layers` of its layers on the graphics card
/// (`extra`: an image encoder, which goes where the layers do; `q8`: the
/// compressed cache).
pub fn need(spec: &ModelSpec, quant: &str, ctx: u32, layers: u32, extra: u64, q8: bool) -> Need {
    let size = spec.variant(quant).map_or(0, |v| v.size);
    let all = (spec.arch.n_layer + 1).max(1);
    let on_gpu = (layers.min(all) as f64 / all as f64).clamp(0.0, 1.0);
    let kv = if q8 { kv_cache_bytes_q8(&spec.arch, ctx) } else { kv_cache_bytes(&spec.arch, ctx) };
    let weights_gpu = (size as f64 * on_gpu) as u64;
    if layers == 0 {
        Need { ram: size + kv + RUNTIME_OVERHEAD + extra, vram: 0 }
    } else {
        // The cache and the work buffers stay on the card with the layers.
        Need { ram: size - weights_gpu + RUNTIME_OVERHEAD / 2, vram: weights_gpu + kv + RUNTIME_OVERHEAD + extra }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct App {
    name: String,
    bytes: u64,
}

/// The question the window shows.
#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    id: String,
    model: String,
    ram_need: u64,
    ram_free: u64,
    ram_total: u64,
    vram_need: u64,
    vram_free: Option<u64>,
    vram_total: Option<u64>,
    short_ram: bool,
    short_vram: bool,
    /// The apps using the most memory, biggest first.
    apps: Vec<App>,
}

/// Whether `need` leaves too little (system memory, graphics memory), given
/// what's free now.
pub fn short(need: Need, ram_free: u64, vram_free: Option<u64>) -> (bool, bool) {
    let ram = need.ram + KEEP_RAM > ram_free;
    let vram = need.vram > 0 && vram_free.is_some_and(|f| need.vram > f + VRAM_SPILL);
    (ram, vram)
}

/// The biggest memory users, by app (all of Chrome's processes together),
/// leaving out SulcusAI and its engine.
fn biggest_apps(sys: &sysinfo::System, skip: &[u32]) -> Vec<App> {
    let mut by_name: HashMap<String, u64> = HashMap::new();
    for (pid, p) in sys.processes() {
        if skip.contains(&pid.as_u32()) {
            continue;
        }
        let name = p.name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if matches!(lower.as_str(), "system" | "registry" | "memory compression" | "memcompression" | "idle" | "secure system") || lower.starts_with("sulcusai") || lower.starts_with("llama-server") {
            continue;
        }
        *by_name.entry(name.trim_end_matches(".exe").trim_end_matches(".EXE").to_string()).or_default() += p.memory();
    }
    let mut apps: Vec<App> = by_name.into_iter().filter(|(_, b)| *b >= 300 * MIB).map(|(name, bytes)| App { name, bytes }).collect();
    apps.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    apps.truncate(4);
    apps
}

static ASKS: LazyLock<Mutex<HashMap<String, oneshot::Sender<bool>>>> = LazyLock::new(Default::default);

/// Before loading `spec`: warns and waits for the person when it would leave
/// too little memory. `replacing`: the model loaded now (its process, its
/// need), which unloads first. Err when they chose not to load it.
pub async fn before_loading(state: &Arc<AppState>, spec: &ModelSpec, want: Need, replacing: Option<(Option<u32>, Need)>) -> Result<(), String> {
    let Some(app) = state.app.get().cloned() else { return Ok(()) };
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let (old_pid, old) = replacing.unwrap_or((None, Need::default()));
    let given_back = old_pid.and_then(|p| sys.process(sysinfo::Pid::from_u32(p))).map_or(0, |p| p.memory());
    let ram_free = sys.available_memory() + given_back;
    let gpu = if want.vram > 0 { crate::perf::nvidia_now().await } else { None };
    let vram_total = gpu.as_ref().and_then(|g| g.mem_total);
    let vram_free = gpu.as_ref().and_then(|g| Some(g.mem_total?.saturating_sub(g.mem_used?) + old.vram));
    let (short_ram, short_vram) = short(want, ram_free, vram_free);
    if !short_ram && !short_vram {
        return Ok(());
    }
    let skip: Vec<u32> = [Some(std::process::id()), old_pid].into_iter().flatten().collect();
    let warning = Warning {
        id: uuid::Uuid::new_v4().simple().to_string(),
        model: spec.name.clone(),
        ram_need: want.ram,
        ram_free,
        ram_total: sys.total_memory(),
        vram_need: want.vram,
        vram_free,
        vram_total,
        short_ram,
        short_vram,
        apps: biggest_apps(&sys, &skip),
    };
    let (tx, rx) = oneshot::channel();
    ASKS.lock().unwrap().insert(warning.id.clone(), tx);
    app.emit("engine:memory", &warning).ok();
    let go = tokio::time::timeout(WAIT, rx).await.ok().and_then(Result::ok).unwrap_or(false);
    ASKS.lock().unwrap().remove(&warning.id);
    app.emit("engine:memory_done", serde_json::json!({ "id": warning.id })).ok();
    state.log("performance", if go { "Loaded a model although memory was short (the user chose to)" } else { "Didn't load a model: memory was short" });
    if go {
        Ok(())
    } else {
        Err(advice(&spec.name, short_ram, short_vram))
    }
}

/// What to do when a model wasn't loaded for want of memory. Closing apps
/// and a smaller window come first: a lower GPU limit moves the model to
/// the processor (slower, and hot on a laptop), and doesn't help at all
/// when system memory is what's short.
fn advice(model: &str, short_ram: bool, short_vram: bool) -> String {
    if short_vram && !short_ram {
        format!("{model} wasn't loaded: the graphics card's memory is in use right now. Close apps using the card (a game, a video editor), or choose a smaller context window (click the meter at the top of the chat), then try again. A smaller model also fits. Lowering the GPU limit at the top of the chat works too, but runs part of the model on the processor: slower and hotter.")
    } else {
        format!("{model} wasn't loaded: the PC is short of memory right now. Close apps you aren't using (the window listed the biggest), or choose a smaller context window (click the meter at the top of the chat), then try again. A smaller model also fits.")
    }
}

/// The person's answer: load it anyway, or not.
#[tauri::command(async)]
pub fn answer_memory_warning(id: String, go: bool) -> Result<(), String> {
    match ASKS.lock().unwrap().remove(&id) {
        Some(tx) => {
            tx.send(go).ok();
            Ok(())
        }
        None => Err("That question is no longer waiting.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_using_the_memory_brings_a_warning() {
        let mut m = crate::catalog::Catalog::bundled().model("qwen3-8b").cloned().expect("Qwen3 8B is in the catalog");
        let quant = m.variants[0].quant.clone();
        m.variants[0].size = 5 * GIB;
        let all = m.arch.n_layer + 1;
        // All on the graphics card: little system memory, the rest on the card.
        let on_card = need(&m, &quant, 8192, all, 0, false);
        assert!(on_card.ram < GIB && on_card.vram > 5 * GIB, "{on_card:?}");
        // The compressed cache takes about half the room on the card.
        let compressed = need(&m, &quant, 8192, all, 0, true);
        assert!(compressed.vram < on_card.vram && compressed.vram > 5 * GIB, "{compressed:?}");
        // On the processor only: it all comes out of system memory.
        let cpu = need(&m, &quant, 8192, 0, 0, false);
        assert!(cpu.vram == 0 && cpu.ram > 5 * GIB, "{cpu:?}");
        // Plenty free: no warning. A game holding most of it: a warning.
        assert_eq!(short(cpu, 24 * GIB, None), (false, false));
        assert_eq!(short(cpu, 6 * GIB, None), (true, false));
        // The card's memory taken by a game: warned about that too.
        assert_eq!(short(on_card, 24 * GIB, Some(12 * GIB)), (false, false));
        assert_eq!(short(on_card, 24 * GIB, Some(3 * GIB)), (false, true));
        // No readout of the card's memory (not NVIDIA): only system memory counts.
        assert_eq!(short(on_card, 24 * GIB, None), (false, false));
    }

    /// The advice puts closing apps and a smaller window before the GPU
    /// limit, and leaves the limit out when system memory is short (moving
    /// the model to the processor needs more of it, not less).
    #[test]
    fn the_advice_puts_the_gpu_limit_last() {
        let card = advice("Qwen3 8B", false, true);
        let (apps, window, limit) = (card.find("Close apps").unwrap(), card.find("smaller context window").unwrap(), card.find("GPU limit").unwrap());
        assert!(apps < limit && window < limit, "{card}");
        assert!(card.contains("slower and hotter"));
        let ram = advice("Qwen3 8B", true, false);
        assert!(ram.contains("Close apps") && ram.contains("smaller context window") && !ram.contains("GPU limit"), "{ram}");
        assert_eq!(advice("Qwen3 8B", true, true), ram);
    }
}

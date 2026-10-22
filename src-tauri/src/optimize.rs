// SPDX-License-Identifier: AGPL-3.0-only
//! "Optimize for this PC": a chat model loaded a few different ways on this
//! PC's graphics card, at the window it's used with: the automatic settings,
//! the whole model on the card with the normal or the compressed cache, and
//! the compressed cache at the estimate's layers. Each is timed, and the
//! fastest that leaves the card room to spare is kept for that model.
//!
//! The automatic settings come from an estimate with a safety margin. Every
//! PC and model is different, and the margin can cost a lot: on a 12 GB RTX
//! 5070 it left 2 of a 14B's 49 layers on the processor (35.6 tokens/s,
//! against 51.8 with all of them on the card).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::catalog::{self, Budget, ModelSpec};
use crate::engine::{self, LaunchSpec, Speed, Tuning};
use crate::{db, AppState};

const KEY: &str = "model_tuning";
const MIB: u64 = 1024 * 1024;

/// Room the card keeps for the desktop and other apps: a setup that leaves
/// less is too tight to keep, however fast.
const HEADROOM: u64 = 600 * MIB;

/// The engine's own memory in system memory beyond this means part of the
/// model spilled off the card (a normal load keeps about 70 MB there).
const SPILL: u64 = 256 * MIB;

/// Within this share of the fastest, the setup that leaves the card more
/// room wins.
const CLOSE: f64 = 0.03;

/// A model's settings measured on this PC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tuned {
    pub quant: String,
    /// The window and the card memory it was measured with: when either
    /// changes (a new window, another mode, a picture reader beside it), the
    /// automatic settings apply again until it's measured again.
    pub ctx: u32,
    pub room: u64,
    pub gpu_layers: u32,
    pub kv_q8: bool,
    pub tps: f64,
    pub prompt_tps: Option<f64>,
    /// The automatic settings' speed in the same run.
    pub before_tps: f64,
    pub all_layers: u32,
    pub at: i64,
    /// A smaller window at which the whole model fits (offered on the card).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smaller_window: Option<WindowOffer>,
}

impl Tuned {
    pub fn applies(&self, quant: &str, ctx: u32, room: u64) -> bool {
        self.quant == quant && self.ctx == ctx && self.room == room
    }
}

pub fn all(conn: &rusqlite::Connection) -> HashMap<String, Tuned> {
    db::get(conn, KEY).unwrap_or_default()
}

pub fn get(conn: &rusqlite::Connection, model_id: &str) -> Option<Tuned> {
    all(conn).remove(model_id)
}

fn save(conn: &rusqlite::Connection, model_id: &str, t: Tuned) -> Result<(), String> {
    let mut map = all(conn);
    map.insert(model_id.to_string(), t);
    db::set(conn, KEY, &map)
}

pub fn forget(conn: &rusqlite::Connection, model_id: &str) -> Result<(), String> {
    let mut map = all(conn);
    if map.remove(model_id).is_some() {
        db::set(conn, KEY, &map)?;
    }
    Ok(())
}

/// One way of loading the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Setup {
    pub gpu_layers: u32,
    pub kv_q8: bool,
}

/// What to try, the automatic settings first. Nothing to try on a PC
/// without a usable graphics card.
pub fn setups(spec: &ModelSpec, quant: &str, b: &Budget, ctx: u32, auto: Setup) -> Vec<Setup> {
    if b.vram == 0 || auto.gpu_layers == 0 {
        return vec![auto];
    }
    let all = spec.arch.n_layer + 1;
    let mut out = vec![auto];
    let mut add = |s: Setup| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    if auto.gpu_layers < all {
        // The estimate's margin may be more than this card needs.
        add(Setup { gpu_layers: all, kv_q8: false });
        add(Setup { gpu_layers: all, kv_q8: true });
        // The compressed cache at the estimate's layers for it.
        if let Some((_, l)) = catalog::with_q8_cache(spec, quant, b, ctx, auto.gpu_layers, false) {
            add(Setup { gpu_layers: l, kv_q8: true });
        }
    } else if auto.kv_q8 {
        // Whole on the card already: the normal cache may still fit (a little faster).
        add(Setup { gpu_layers: all, kv_q8: false });
    }
    out
}

/// One setup's outcome.
#[derive(Debug, Clone, Serialize)]
pub struct Trial {
    pub setup: Setup,
    pub speed: Option<Speed>,
    /// The card's memory left free with it loaded (None: couldn't read it).
    pub free: Option<u64>,
    /// Why it wasn't kept (didn't load, too tight, too hot).
    pub problem: Option<String>,
}

/// The fastest that loaded and left room; one within `CLOSE` of it that
/// leaves more of the card free wins. None when nothing worked.
pub fn best(trials: &[Trial]) -> Option<&Trial> {
    let ok: Vec<&Trial> = trials.iter().filter(|t| t.problem.is_none() && t.speed.is_some()).collect();
    let top = ok.iter().map(|t| t.speed.as_ref().map_or(0.0, |s| s.tps)).fold(0.0, f64::max);
    ok.into_iter()
        .filter(|t| t.speed.as_ref().is_some_and(|s| s.tps >= top * (1.0 - CLOSE)))
        .max_by_key(|t| (t.free.unwrap_or(0), u32::MAX - t.setup.gpu_layers))
}

/// A smaller window at which the whole model fits the card, and how fast
/// it writes there: offered, never applied (the window is the person's).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowOffer {
    pub ctx: u32,
    pub tps: f64,
}

/// What the run found.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub model_id: String,
    pub ctx: u32,
    pub before_tps: f64,
    pub tps: f64,
    pub gpu_layers: u32,
    pub all_layers: u32,
    pub kv_q8: bool,
    pub trials: Vec<Trial>,
    pub smaller_window: Option<WindowOffer>,
}

/// Smaller windows to offer when the whole model doesn't fit at `ctx`.
pub fn smaller_windows(ctx: u32) -> Vec<u32> {
    [12_288u32, 8192].into_iter().filter(|c| *c < ctx).collect()
}

/// Waits for the card to cool between setups (each load and test heats
/// it): to 65 °C, at most 90 s. Over 85 °C it stops trying.
async fn cool(state: &AppState) -> Result<(), String> {
    crate::perf::cool_down(&state.limits(), |_| {}).await;
    let started = Instant::now();
    loop {
        let t = crate::perf::temps().await.gpu;
        match t {
            Some(c) if c >= 85.0 => return Err(format!("The graphics card reached {c:.0} °C, so it stopped trying more setups.")),
            Some(c) if c > 65.0 && started.elapsed() < Duration::from_secs(90) => tokio::time::sleep(Duration::from_secs(5)).await,
            _ => return Ok(()),
        }
    }
}

/// What every attempt in one run shares.
struct Shared<'a> {
    state: &'a Arc<AppState>,
    spec: &'a ModelSpec,
    quant: &'a str,
    exe: &'a std::path::Path,
    model_path: &'a std::path::Path,
    mmproj: Option<&'a std::path::Path>,
    log: &'a std::path::Path,
    limits: &'a crate::perf::Limits,
    threads: usize,
    batch: u32,
    auto: Setup,
    all_layers: u32,
    card: Option<i64>,
    total: u64,
}

/// One setup at a window: loaded, its room checked, its speed measured.
/// The flag: stop trying (the card got too hot).
async fn attempt(r: &Shared<'_>, engine: &mut engine::Engine, s: Setup, ctx: u32) -> (Trial, bool) {
    let label = format!(
        "{} of {} layers on the graphics card, {} cache, {}K window",
        s.gpu_layers.min(r.all_layers),
        r.all_layers,
        if s.kv_q8 { "compressed" } else { "normal" },
        ctx / 1024
    );
    if let Err(why) = cool(r.state).await {
        return (Trial { setup: s, speed: None, free: None, problem: Some(why) }, true);
    }
    engine.stop().await;
    let cpu_moe = if s.gpu_layers < r.all_layers { catalog::moe_offload(r.spec, r.quant, s.gpu_layers) } else { None };
    // Wholly on the card, the mode's own thread count (as chats load it).
    let threads = if s.gpu_layers >= r.all_layers { r.limits.threads.max(r.threads) } else { r.threads };
    let loaded = engine
        .ensure_tuned(
            LaunchSpec {
                exe: r.exe,
                model_path: r.model_path,
                model_id: &r.spec.id,
                quant: r.quant,
                ctx,
                gpu_layers: s.gpu_layers,
                threads,
                low_priority: r.limits.low_priority,
                log: r.log,
                mmproj: r.mmproj,
                batch: r.batch,
                lora: None,
            },
            Tuning { cpu_moe, kv_q8: s.kv_q8, think_budget: None },
            r.state.job(),
        )
        .await;
    let ep = match loaded {
        Ok(ep) => ep,
        Err(e) => {
            crate::applog::line("model", &format!("Optimizing {}: {label} didn't load ({})", r.spec.name, e.lines().next().unwrap_or("")));
            return (Trial { setup: s, speed: None, free: None, problem: Some("It didn't fit on the graphics card.".into()) }, false);
        }
    };
    // Loaded. Part of it in system memory (Windows' quiet overflow) is too
    // slow for long chats, however fast the short test.
    tokio::time::sleep(Duration::from_secs(2)).await;
    if let Some((_, shared)) = engine.loaded().and_then(|l| l.0).and_then(crate::perf::process_card_memory) {
        if shared > SPILL {
            crate::applog::line("model", &format!("Optimizing {}: {label} put {} MB in system memory", r.spec.name, shared / MIB));
            return (Trial { setup: s, speed: None, free: None, problem: Some("It didn't fit on the graphics card: part of it went to system memory, which makes long chats slow.".into()) }, false);
        }
    }
    // How much of the card is left for everything else.
    let free = crate::perf::card_memory_used(r.card).filter(|_| r.total > 0).map(|used| r.total.saturating_sub(used));
    if s != r.auto && free.is_some_and(|f| f < HEADROOM) {
        crate::applog::line("model", &format!("Optimizing {}: {label} left only {} MB of the card free", r.spec.name, free.unwrap_or(0) / MIB));
        return (Trial { setup: s, speed: None, free, problem: Some("It left too little of the graphics card free for other apps.".into()) }, false);
    }
    match engine::measure(&ep).await {
        Ok(sp) => {
            crate::applog::line(
                "model",
                &format!("Optimizing {}: {label} wrote {:.1} tokens/s{}", r.spec.name, sp.tps, free.map(|f| format!(", {} MB of the card free", f / MIB)).unwrap_or_default()),
            );
            (Trial { setup: s, speed: Some(sp), free, problem: None }, false)
        }
        Err(e) => (Trial { setup: s, speed: None, free, problem: Some(format!("The test reply failed: {e}")) }, false),
    }
}

/// Measures the setups for an installed chat model and keeps the best.
/// Holds the engine the whole time (a chat asking for a model waits), and
/// unloads at the end so the card is free again.
pub async fn run(state: &Arc<AppState>, model_id: &str, progress: &(dyn Fn(usize, usize, &str) + Sync)) -> Result<Report, String> {
    if !state.generations.lock().unwrap().is_empty() {
        return Err("Wait for the reply to finish, then optimize.".into());
    }
    if crate::finetune::training() || crate::media::any_job() {
        return Err("The graphics card is busy making something. Try again when it's done.".into());
    }
    let installed = db::installed_model(&state.db.lock().unwrap(), model_id).ok_or("That model isn't installed.")?;
    let spec = state.model_spec(model_id).ok_or("That model isn't available anymore.")?;
    let backend = engine::backend_for(&state.budget())?;
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, backend).ok_or("The AI engine isn't installed yet: send a chat message once first.")?;
    let model_path = crate::model_file(&state.paths, &installed);
    let mmproj = crate::vision_file(&state.paths, &spec);
    let encoder = mmproj.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
    let plan = crate::plan_with(state, &spec, &installed.quant, encoder, false);
    let auto = Setup { gpu_layers: plan.gpu_layers, kv_q8: plan.tuning.kv_q8 };
    let list = setups(&spec, &installed.quant, &plan.budget, plan.ctx, auto);
    let card = crate::hardware::dedicated().and_then(|c| c.luid);
    let total = state.hardware.read().unwrap().best_gpu().map_or(0, |g| g.vram_bytes);
    let all_layers = spec.arch.n_layer + 1;
    let log = state.paths.engine_log();
    let limits = state.limits();

    let mut engine = state.engine.lock().await;
    let shared = Shared {
        state,
        spec: &spec,
        quant: &installed.quant,
        exe: &exe,
        model_path: &model_path,
        mmproj: mmproj.as_deref(),
        log: &log,
        limits: &limits,
        threads: plan.threads,
        batch: plan.batch,
        auto,
        all_layers,
        card,
        total,
    };
    let windows = smaller_windows(plan.ctx);
    let steps = list.len() + usize::from(!windows.is_empty());
    let mut trials: Vec<Trial> = Vec::new();
    let mut hot = false;
    for (i, s) in list.iter().enumerate() {
        progress(i, steps, &format!("{} of {} layers on the graphics card, {} cache", s.gpu_layers.min(all_layers), all_layers, if s.kv_q8 { "compressed" } else { "normal" }));
        if !state.generations.lock().unwrap().is_empty() {
            break;
        }
        let (t, stop) = attempt(&shared, &mut engine, *s, plan.ctx).await;
        trials.push(t);
        if stop {
            hot = true;
            break;
        }
    }
    // The whole model didn't fit at this window: how fast it would be at a
    // smaller one where it does (offered, not applied).
    let mut smaller_window = None;
    let whole = |t: &Trial| t.problem.is_none() && t.speed.is_some() && t.setup.gpu_layers >= all_layers;
    if !hot && !windows.is_empty() && !trials.iter().any(whole) && state.generations.lock().unwrap().is_empty() {
        progress(list.len(), steps, "the whole model at a smaller window");
        for w in &windows {
            let (t, stop) = attempt(&shared, &mut engine, Setup { gpu_layers: all_layers, kv_q8: true }, *w).await;
            if let (None, Some(sp)) = (&t.problem, &t.speed) {
                smaller_window = Some(WindowOffer { ctx: *w, tps: sp.tps });
                break;
            }
            if stop {
                break;
            }
        }
    }
    engine.stop().await;
    drop(engine);

    let before = trials.iter().find(|t| t.setup == auto).and_then(|t| t.speed.as_ref()).map_or(0.0, |s| s.tps);
    let chosen = best(&trials).cloned().ok_or("None of the setups worked on this PC; it keeps the automatic settings.")?;
    let speed = chosen.speed.clone().unwrap_or_default();
    {
        let conn = state.db.lock().unwrap();
        save(
            &conn,
            model_id,
            Tuned {
                quant: installed.quant.clone(),
                ctx: plan.ctx,
                room: plan.budget.vram,
                gpu_layers: chosen.setup.gpu_layers,
                kv_q8: chosen.setup.kv_q8,
                tps: speed.tps,
                prompt_tps: speed.prompt_tps,
                before_tps: before,
                all_layers,
                at: db::now_ms(),
                smaller_window: smaller_window.clone(),
            },
        )?;
        db::set_speed(&conn, model_id, speed.tps, speed.prompt_tps)?;
    }
    state.log("model", &format!("Optimized {} for this PC: {:.1} tokens/s (the automatic settings {:.1})", spec.name, speed.tps, before));
    Ok(Report {
        model_id: model_id.to_string(),
        ctx: plan.ctx,
        before_tps: before,
        tps: speed.tps,
        gpu_layers: chosen.setup.gpu_layers.min(all_layers),
        all_layers,
        kv_q8: chosen.setup.kv_q8,
        trials,
        smaller_window,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speed(tps: f64) -> Option<Speed> {
        Some(Speed { tps, prompt_tps: None })
    }

    #[test]
    fn the_fastest_wins_unless_another_is_as_fast_with_more_room() {
        let s = |gpu_layers, kv_q8| Setup { gpu_layers, kv_q8 };
        let trials = vec![
            Trial { setup: s(39, false), speed: speed(15.5), free: Some(2000 * MIB), problem: None },
            Trial { setup: s(49, false), speed: speed(51.8), free: Some(700 * MIB), problem: None },
            Trial { setup: s(49, true), speed: speed(50.9), free: Some(1500 * MIB), problem: None },
            Trial { setup: s(47, true), speed: speed(60.0), free: Some(100 * MIB), problem: Some("too tight".into()) },
        ];
        // 50.9 is within 3% of 51.8 and leaves more of the card free.
        assert_eq!(best(&trials).unwrap().setup, s(49, true));
        assert!(best(&[Trial { setup: s(1, false), speed: None, free: None, problem: Some("no".into()) }]).is_none());
    }

    #[test]
    fn it_tries_the_whole_model_on_the_card_with_either_cache() {
        let c = catalog::Catalog::bundled();
        let m = c.model("qwen3-14b").unwrap();
        let v = m.variant("Q4_K_M").unwrap();
        let all = m.arch.n_layer + 1;
        let b = Budget { vram: v.size + catalog::kv_cache_bytes_q8(&m.arch, 16_384) + catalog::RUNTIME_OVERHEAD, ram: 32 << 30, disk: 1 << 40, gpu_bw_gbs: 380.0, cpu_bw_gbs: 50.0 };
        let list = setups(m, "Q4_K_M", &b, 16_384, Setup { gpu_layers: 30, kv_q8: false });
        assert_eq!(list[0], Setup { gpu_layers: 30, kv_q8: false }, "the automatic settings first");
        assert!(list.contains(&Setup { gpu_layers: all, kv_q8: false }) && list.contains(&Setup { gpu_layers: all, kv_q8: true }));
        // Already whole on the card with the compressed cache: the normal one is tried too.
        assert_eq!(setups(m, "Q4_K_M", &b, 8192, Setup { gpu_layers: all, kv_q8: true }).len(), 2);
        // No graphics card: nothing to try but the automatic settings.
        let cpu = Budget { vram: 0, ..b };
        assert_eq!(setups(m, "Q4_K_M", &cpu, 8192, Setup { gpu_layers: 0, kv_q8: false }).len(), 1);
    }

    #[test]
    fn measured_settings_apply_only_where_they_were_measured() {
        let t = Tuned { quant: "Q4_K_M".into(), ctx: 16_384, room: 11 << 30, gpu_layers: 47, kv_q8: true, tps: 33.8, prompt_tps: None, before_tps: 15.5, all_layers: 49, at: 0, smaller_window: None };
        assert!(t.applies("Q4_K_M", 16_384, 11 << 30));
        assert!(!t.applies("Q4_K_M", 8192, 11 << 30), "another window");
        assert!(!t.applies("Q4_K_M", 16_384, 10 << 30), "another mode or a picture reader beside it");
        assert!(!t.applies("Q8_0", 16_384, 11 << 30), "another version");
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Where the chat model runs (the graphics card, the processor, or both)
//! and why it isn't all on the card. The status bar shows it, so a model
//! running on the processor of a PC with a graphics card (slow, and hot on
//! a laptop) always comes with its cause and, where there is one, a fix.

use serde::Serialize;

use crate::catalog::{self, Budget, ModelSpec};
use crate::hardware::{BrokenCard, Hardware};
use crate::perf::{self, Mode, PerfSettings};
use crate::{AppState, AppStateRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// The whole model on the graphics card.
    Card,
    /// Some of it on the processor.
    Partly,
    /// All of it on the processor.
    Processor,
}

/// Why some or all of the model runs on the processor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reason {
    /// "no_card", "no_driver", "engine_no_card", "gpu_layers", "gpu_limit",
    /// "cool_on_battery", "cool_mode", "too_small" or "window".
    pub code: &'static str,
    /// In English, for the app log; the window words it from `code`.
    pub message: String,
    /// What the window offers to fix it: "gpu_full" (GPU limit to 100%),
    /// "recheck" (check the graphics card again), "perf_settings" or
    /// "advanced_settings".
    pub fix: Option<&'static str>,
    /// The card it's about (a working card, or one without a driver), and
    /// its maker when Windows names the card only by its stand-in driver.
    pub card: Option<String>,
    pub maker: Option<String>,
    /// The GPU limit (percent), the layers Advanced or the card allow, or
    /// Device Manager's problem code.
    pub number: Option<u32>,
    /// "window": how much the chat keeps in mind (thousands of tokens).
    pub ctx_k: Option<u32>,
}

/// What decides where the model runs, gathered from the PC and the settings.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The dedicated graphics card found, if any.
    pub card: Option<String>,
    /// Integrated graphics found (models don't use them).
    pub integrated: Option<String>,
    /// Cards Windows lists that aren't working.
    pub broken: Vec<BrokenCard>,
    /// Whether the engine saw a card it may use (None: not asked yet).
    pub engine_sees_card: Option<bool>,
    pub gpu_limit: u8,
    /// Cool & quiet runs because the PC is on battery, and keeps layers off the card.
    pub cool_on_battery: bool,
    /// Cool & quiet was chosen, and keeps layers off the card.
    pub cool_mode: bool,
    /// Layers set in Advanced.
    pub advanced_layers: Option<u32>,
    /// The most layers this PC's card takes of this model with nothing
    /// holding it back (None: not known).
    pub card_can_take: Option<u32>,
    /// Layers planned for the card, and the model's layers in all.
    pub planned: u32,
    pub total: u32,
    /// Layers on the card as the engine reported them (None: not known).
    pub actual: Option<u32>,
    /// A mixture-of-experts model whose experts wait in system memory.
    pub experts_in_memory: bool,
    /// How much the running model keeps in mind (its context window).
    pub ctx: u32,
}

pub fn place(f: &Facts) -> Place {
    let on_card = f.actual.unwrap_or(f.planned);
    if on_card == 0 {
        Place::Processor
    } else if on_card >= f.total && !f.experts_in_memory {
        Place::Card
    } else {
        Place::Partly
    }
}

fn reason(code: &'static str, message: impl Into<String>, fix: Option<&'static str>) -> Reason {
    Reason { code, message: message.into(), fix, card: None, maker: None, number: None, ctx_k: None }
}

/// Why the model isn't wholly on the graphics card, most decisive first.
/// Empty when it is.
pub fn reasons(f: &Facts) -> Vec<Reason> {
    let mut out = Vec::new();
    if place(f) == Place::Card {
        return out;
    }
    // No card to use: nothing else matters.
    let Some(card) = f.card.clone() else {
        for b in &f.broken {
            let maker = if b.vendor.is_empty() { String::new() } else { format!("{} ", b.vendor) };
            let state = match b.code {
                22 => "turned off in Device Manager",
                28 | 0 => "missing its driver",
                _ => "stopped by Windows",
            };
            let mut r = reason(
                "no_driver",
                format!("Windows lists a {maker}graphics card ({}) that is {state}, so models run on the processor. Install the card's driver from its maker (or the laptop maker), then check the graphics card again.", b.name),
                Some("recheck"),
            );
            r.card = Some(b.name.clone());
            r.maker = (!b.vendor.is_empty()).then(|| b.vendor.clone());
            r.number = Some(b.code);
            out.push(r);
        }
        if f.broken.is_empty() {
            let mut r = reason(
                "no_card",
                match &f.integrated {
                    Some(name) => format!("Only built-in graphics were found ({name}), and models don't use those. If this PC has a separate graphics card, it may be switched off (some laptops have an eco or built-in-only graphics mode) or missing its driver."),
                    None => String::from("No graphics card was found, so models run on the processor."),
                },
                Some("recheck"),
            );
            r.card = f.integrated.clone();
            out.push(r);
        }
        return out;
    };
    // The engine didn't see the card, or was asked to use it and didn't.
    if f.engine_sees_card == Some(false) || (f.planned > 0 && f.actual == Some(0)) {
        let mut r = reason(
            "engine_no_card",
            format!("The engine couldn't use {card}: its Vulkan driver didn't offer it. Updating the graphics driver from the card's maker usually fixes this."),
            Some("recheck"),
        );
        r.card = Some(card.clone());
        out.push(r);
    }
    if let Some(n) = f.advanced_layers.filter(|n| *n < f.total) {
        let mut r = reason("gpu_layers", format!("Advanced settings put {n} of {} layers on the graphics card.", f.total), Some("advanced_settings"));
        r.number = Some(n);
        out.push(r);
    }
    if f.gpu_limit < 100 {
        let mut r = reason(
            "gpu_limit",
            if f.gpu_limit == 0 { String::from("The GPU limit at the top of the chat is Off, so the model runs on the processor only.") } else { format!("The GPU limit at the top of the chat is {}%, so the rest runs on the processor.", f.gpu_limit) },
            Some("gpu_full"),
        );
        r.number = Some(f.gpu_limit as u32);
        out.push(r);
    }
    if f.cool_on_battery {
        out.push(reason("cool_on_battery", "Cool & quiet runs while the laptop is on battery and leaves part of the graphics card free.", Some("perf_settings")));
    } else if f.cool_mode {
        out.push(reason("cool_mode", "Cool & quiet mode leaves part of the graphics card free.", Some("perf_settings")));
    }
    if let Some(can) = f.card_can_take.filter(|n| *n < f.total || f.experts_in_memory) {
        let mut r = reason(
            "too_small",
            if can == 0 {
                format!("{card} is too small for this model. A smaller model, or a smaller context window, fits it.")
            } else if can >= f.total {
                format!("{card} holds every layer of this model but not all its experts; those wait in system memory. A smaller model fits it whole.")
            } else {
                format!("{card} holds {can} of this model's {} layers; the rest runs on the processor. A smaller model, or a smaller context window, fits it whole.", f.total)
            },
            None,
        );
        r.card = Some(card.clone());
        r.number = Some(can);
        out.push(r);
    }
    // Nothing else holds it back and the card takes it whole at the
    // automatic window: a bigger window (chosen, or the 16K tool models
    // get) moved layers off to make room for what it keeps in mind.
    if out.is_empty() && f.card_can_take.is_some_and(|n| n >= f.total) && f.planned < f.total && f.ctx > 0 {
        let k = f.ctx / 1024;
        let mut r = reason(
            "window",
            format!("This chat keeps {k}K of conversation in mind, so {card} holds {} of the model's {} layers and the rest runs on the processor. A smaller memory setting fits it whole.", f.planned, f.total),
            None,
        );
        r.card = Some(card);
        r.number = Some(f.planned);
        r.ctx_k = Some(k);
        out.push(r);
    }
    out
}

/// For the cooling pause's text: why the model runs on the processor, in a
/// few words ("the GPU limit is Off").
pub fn short(r: &Reason) -> String {
    match r.code {
        "no_driver" => "the graphics card has no working driver".into(),
        "no_card" => "no usable graphics card was found".into(),
        "engine_no_card" => "the engine couldn't use the graphics card (update its driver)".into(),
        "gpu_layers" => format!("Advanced settings put {} layers on the graphics card", r.number.unwrap_or(0)),
        "gpu_limit" if r.number == Some(0) => "the GPU limit is Off".into(),
        "gpu_limit" => format!("the GPU limit is {}%", r.number.unwrap_or(0)),
        "cool_on_battery" => "Cool & quiet runs on battery".into(),
        "cool_mode" => "Cool & quiet mode is on".into(),
        "window" => "the chat keeps more in mind than the card has room for".into(),
        _ => "the graphics card is too small for this model".into(),
    }
}

/// The layers launched with these settings, before a picture reader or a
/// chosen window (only to compare settings with each other).
fn layers_with(spec: &ModelSpec, quant: &str, hw: &Hardware, s: &PerfSettings, on_battery: bool) -> u32 {
    let l = perf::limits(s, hw, on_battery);
    catalog::launch_within(spec, quant, &Budget::from_hardware(hw), &l).1
}

/// The most layers the card takes of this model with nothing holding it
/// back (Balanced, plugged in, the whole card), the compressed cache and a
/// smaller window included.
pub fn card_can_take(spec: &ModelSpec, quant: &str, hw: &Hardware) -> u32 {
    let full = Budget::from_hardware(hw);
    let ideal = perf::limits(&PerfSettings { mode: Mode::Balanced, cool_on_battery: false, gpu_limit: 100, ..Default::default() }, hw, false);
    let (ctx, layers) = catalog::launch_within(spec, quant, &full, &ideal);
    let room = full.with_limits(&ideal);
    let mut best = layers;
    if let Some((_, n)) = catalog::with_q8_cache(spec, quant, &room, ctx, layers, false) {
        best = best.max(n);
    }
    if best == 0 {
        if let Some((_, n, _)) = catalog::on_small_card(spec, quant, &room, ctx, true) {
            best = n;
        }
    }
    best
}

/// The settings' part in where it runs: Cool & quiet (on battery or
/// chosen), when it costs layers. (Heat never moves layers off the card.)
fn setting_facts(f: &mut Facts, spec: &ModelSpec, quant: &str, hw: &Hardware, s: &PerfSettings, on_battery: bool) {
    let full = PerfSettings { gpu_limit: 100, ..s.clone() };
    let cool_now = perf::limits(&full, hw, on_battery).mode == Mode::Cool;
    if cool_now {
        let balanced = PerfSettings { mode: Mode::Balanced, cool_on_battery: false, ..full.clone() };
        let costs = layers_with(spec, quant, hw, &balanced, on_battery) > layers_with(spec, quant, hw, &full, on_battery);
        if s.mode == Mode::Cool {
            f.cool_mode = costs;
        } else {
            f.cool_on_battery = costs;
        }
    }
}

/// Everything `reasons` needs, for the model loaded now.
fn facts(state: &AppState, spec: &ModelSpec, quant: &str, planned: u32, experts: bool, actual: Option<u32>, total: Option<u32>, ctx: u32) -> Facts {
    let hw = state.hardware.read().unwrap().clone();
    let s = perf::settings(&state.db.lock().unwrap());
    let on_battery = crate::hardware::on_battery() == Some(true);
    let total = total.unwrap_or(spec.arch.n_layer + 1);
    let mut f = Facts {
        card: hw.best_gpu().map(|g| g.name.clone()),
        integrated: hw.gpus.iter().find(|g| g.integrated).map(|g| g.name.clone()),
        broken: hw.broken_cards.clone(),
        engine_sees_card: crate::engine::devices_seen().map(|d| crate::engine::sees_a_card(&d, &hw.gpus)),
        gpu_limit: s.gpu_limit,
        advanced_layers: crate::advanced::get(state).engine.gpu_layers,
        planned,
        total,
        actual,
        experts_in_memory: experts,
        ctx,
        ..Default::default()
    };
    if f.card.is_some() {
        setting_facts(&mut f, spec, quant, &hw, &s, on_battery);
        f.card_can_take = Some(card_can_take(spec, quant, &hw));
    }
    f
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct WhereItRuns {
    /// None while no model is loaded (or one is loading).
    pub place: Option<Place>,
    pub model_id: Option<String>,
    /// Layers on the graphics card (as the engine reported them, else as
    /// planned), and the model's layers in all.
    pub layers_on_card: Option<u32>,
    pub layers_total: Option<u32>,
    /// The engine's own report, not just the plan.
    pub reported: bool,
    /// The card it runs on, as the engine named it.
    pub device: Option<String>,
    pub reasons: Vec<Reason>,
}

/// Where the loaded model runs and why, without waiting for a model that
/// is loading (None then).
pub fn now(state: &AppState) -> Option<WhereItRuns> {
    let (model_id, quant, planned, cpu_moe, placed, ctx) = {
        let mut eng = state.engine.try_lock().ok()?;
        let (model_id, quant, planned, cpu_moe, placed) = eng.placement()?;
        let ctx = eng.loaded().map_or(0, |l| l.3);
        (model_id, quant, planned, cpu_moe, placed, ctx)
    };
    let spec = state.model_spec(&model_id)?;
    let f = facts(state, &spec, &quant, planned, cpu_moe.is_some(), placed.on_card(), placed.total(), ctx);
    Some(WhereItRuns {
        place: Some(place(&f)),
        model_id: Some(model_id),
        layers_on_card: Some(f.actual.unwrap_or(f.planned).min(f.total)),
        layers_total: Some(f.total),
        reported: f.actual.is_some(),
        device: placed.device(),
        reasons: reasons(&f),
    })
}

/// A graphics card as the status bar's menu lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CardView {
    pub name: String,
    pub vendor: String,
    /// Its own memory (built-in graphics share system memory instead).
    pub memory: u64,
    /// "card" (models can use it), "built_in" (integrated graphics, not
    /// used for models) or "no_driver" (listed by Windows but not working).
    pub kind: &'static str,
    /// The card models use: the one picked, or the automatic choice.
    pub main: bool,
    /// Device Manager's problem code, for one that isn't working.
    pub code: Option<u32>,
}

/// Every graphics card found: the ones models can use (largest first),
/// then built-in graphics, then cards that aren't working.
pub fn cards_of(hw: &Hardware) -> Vec<CardView> {
    let main = hw.best_gpu().map(|g| g.name.clone());
    let mut dedicated: Vec<&crate::hardware::Gpu> = hw.gpus.iter().filter(|g| !g.integrated).collect();
    dedicated.sort_by(|a, b| b.vram_bytes.cmp(&a.vram_bytes));
    let mut out: Vec<CardView> = dedicated
        .into_iter()
        .map(|g| CardView { name: g.name.clone(), vendor: g.vendor.clone(), memory: g.vram_bytes, kind: "card", main: main.as_deref() == Some(g.name.as_str()), code: None })
        .collect();
    out.extend(hw.gpus.iter().filter(|g| g.integrated).map(|g| CardView { name: g.name.clone(), vendor: g.vendor.clone(), memory: g.vram_bytes, kind: "built_in", main: false, code: None }));
    out.extend(hw.broken_cards.iter().map(|b| CardView { name: b.name.clone(), vendor: b.vendor.clone(), memory: 0, kind: "no_driver", main: false, code: Some(b.code) }));
    out
}

/// What the status bar's graphics card menu shows.
#[derive(Debug, Clone, Serialize, Default)]
pub struct GpuMenu {
    pub cards: Vec<CardView>,
    /// The card picked by name, when it's here (None: automatic).
    pub chosen: Option<String>,
    /// Where the loaded model runs, and why not all on the card.
    pub runs: WhereItRuns,
}

fn menu(state: &AppState) -> GpuMenu {
    let hw = state.hardware.read().unwrap().clone();
    let chosen = crate::hardware::main_choice().filter(|n| hw.gpus.iter().any(|g| !g.integrated && &g.name == n));
    GpuMenu { cards: cards_of(&hw), chosen, runs: now(state).unwrap_or_default() }
}

/// The status bar's graphics card menu.
#[tauri::command(async)]
pub fn gpu_menu(state: AppStateRef) -> GpuMenu {
    menu(&state)
}

/// Makes a card the one models run on (by its name; None: automatic, the
/// card with the most memory). Its memory is what models are fitted to,
/// and the engine is told to use it. A model loaded now starts again on it,
/// unless a reply is using it (then from the next reply).
#[tauri::command]
pub async fn set_main_gpu(app: tauri::AppHandle, state: AppStateRef<'_>, name: Option<String>) -> Result<GpuMenu, String> {
    use tauri::Emitter;
    let state = state.inner().clone();
    if let Some(n) = &name {
        if !state.hardware.read().unwrap().gpus.iter().any(|g| !g.integrated && &g.name == n) {
            return Err("That graphics card wasn't found. Scan for graphics cards, then pick it again.".into());
        }
    }
    {
        let conn = state.db.lock().unwrap();
        crate::db::set(&conn, "main_gpu", &name)?;
        let what = match &name {
            Some(n) => format!("Models run on {n}"),
            None => String::from("Models run on the graphics cards together (the biggest one's maker)"),
        };
        crate::db::log_action(&conn, "performance", &what);
        crate::applog::line("app", &what);
    }
    crate::hardware::set_main(name.clone());
    state.hardware.write().unwrap().main = name.clone();
    let reload = if state.generations.lock().unwrap().is_empty() { state.engine.try_lock().ok().and_then(|mut e| e.asked()) } else { None };
    let reloading = reload.is_some();
    if let Some(id) = reload {
        let (s, app) = (state.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            let done = crate::llm_endpoint(&s, Some(id), false, |_| {}).await;
            if let Err(e) = &done {
                crate::applog::line("engine", &format!("Couldn't start the model again on the picked card: {e}"));
            }
            app.emit("gpu:main", serde_json::json!({ "reloaded": done.is_ok() })).ok();
        });
    }
    app.emit("gpu:main", serde_json::json!({ "reloading": reloading })).ok();
    Ok(menu(&state))
}

/// For the cooling pause while the model runs (partly) on the processor of
/// a PC that has a graphics card: why it does, in a few words.
pub fn processor_note(state: &AppState) -> Option<String> {
    let w = now(state)?;
    let hw = state.hardware.read().unwrap().clone();
    if hw.best_gpu().is_none() && hw.broken_cards.is_empty() {
        return None;
    }
    let first = w.reasons.first()?;
    Some(match w.place? {
        Place::Card => return None,
        Place::Partly => format!("part of the model runs on the processor because {}", short(first)),
        Place::Processor => format!("the model runs on the processor because {}", short(first)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laptop() -> Facts {
        Facts { card: Some("NVIDIA GeForce RTX 3050 Laptop GPU".into()), integrated: Some("Intel(R) UHD Graphics".into()), engine_sees_card: Some(true), gpu_limit: 100, card_can_take: Some(37), planned: 37, total: 37, actual: Some(37), ..Default::default() }
    }

    fn codes(f: &Facts) -> Vec<&'static str> {
        reasons(f).iter().map(|r| r.code).collect()
    }

    /// The menu lists every card found: the usable ones (the main one
    /// marked), the built-in graphics, and a card without its driver.
    #[test]
    fn the_menu_lists_every_card_found() {
        use crate::hardware::Gpu;
        let gpu = |name: &str, gb: u64, integrated: bool| Gpu { name: name.into(), vendor: "NVIDIA".into(), vram_bytes: gb << 30, integrated, luid: None, dxgi: None };
        let mut hw = Hardware {
            gpus: vec![gpu("Intel(R) UHD Graphics", 0, true), gpu("NVIDIA GeForce RTX 4060", 8, false), gpu("NVIDIA GeForce RTX 3060", 12, false)],
            broken_cards: vec![BrokenCard { name: "Microsoft Basic Display Adapter".into(), vendor: "AMD".into(), code: 0 }],
            ..Default::default()
        };
        let kinds = |c: &[CardView]| c.iter().map(|c| (c.name.clone(), c.kind, c.main)).collect::<Vec<_>>();
        assert_eq!(
            kinds(&cards_of(&hw)),
            [
                ("NVIDIA GeForce RTX 3060".to_string(), "card", true),
                ("NVIDIA GeForce RTX 4060".to_string(), "card", false),
                ("Intel(R) UHD Graphics".to_string(), "built_in", false),
                ("Microsoft Basic Display Adapter".to_string(), "no_driver", false),
            ]
        );
        hw.main = Some("NVIDIA GeForce RTX 4060".into());
        assert!(cards_of(&hw).iter().any(|c| c.name == "NVIDIA GeForce RTX 4060" && c.main), "the picked card is marked");
        assert_eq!(cards_of(&hw).iter().filter(|c| c.main).count(), 1);
    }

    #[test]
    fn all_on_the_card_needs_no_reason() {
        assert_eq!(place(&laptop()), Place::Card);
        assert!(reasons(&laptop()).is_empty());
        // Experts kept in system memory: partly on the processor, by plan.
        let moe = Facts { experts_in_memory: true, card_can_take: Some(49), planned: 49, total: 49, actual: Some(49), ..laptop() };
        assert_eq!(place(&moe), Place::Partly);
        assert_eq!(codes(&moe), ["too_small"]);
    }

    /// The card takes it whole at the automatic window, but the chat's
    /// bigger window moved layers off: said, not left blank.
    #[test]
    fn a_bigger_window_that_moved_layers_off_is_named() {
        let big = Facts { planned: 30, actual: Some(30), ctx: 32 * 1024, ..laptop() };
        assert_eq!(place(&big), Place::Partly);
        let r = reasons(&big);
        assert_eq!(r.iter().map(|r| r.code).collect::<Vec<_>>(), ["window"]);
        assert_eq!((r[0].number, r[0].ctx_k), (Some(30), Some(32)));
        // Another cause comes first, and alone.
        assert_eq!(codes(&Facts { gpu_limit: 50, ..big }), ["gpu_limit"]);
    }

    /// A laptop with Intel graphics plus a dedicated card, the model on the
    /// processor: each cause gets its own reason and fix.
    #[test]
    fn each_cause_is_named_with_its_fix() {
        // The dedicated card has no driver: Windows shows a Basic Display Adapter.
        let broken = Facts {
            card: None,
            broken: vec![BrokenCard { name: "Microsoft Basic Display Adapter".into(), vendor: "NVIDIA".into(), code: 0 }],
            planned: 0,
            actual: Some(0),
            card_can_take: None,
            ..laptop()
        };
        let r = reasons(&broken);
        assert_eq!((r[0].code, r[0].fix), ("no_driver", Some("recheck")));
        assert_eq!((r[0].card.as_deref(), r[0].maker.as_deref()), (Some("Microsoft Basic Display Adapter"), Some("NVIDIA")));
        assert!(r[0].message.contains("missing its driver"), "{}", r[0].message);
        assert_eq!(r.len(), 1, "limits don't matter without a card");

        // No card at all, only the built-in graphics.
        let none = Facts { broken: vec![], ..broken.clone() };
        let r = reasons(&none);
        assert_eq!(r[0].code, "no_card");
        assert!(r[0].message.contains("Intel(R) UHD Graphics"));

        // The GPU limit at Off, and at 50%.
        let off = Facts { gpu_limit: 0, planned: 0, actual: Some(0), ..laptop() };
        assert_eq!(place(&off), Place::Processor);
        let r = reasons(&off);
        assert_eq!((r[0].code, r[0].fix, r[0].number), ("gpu_limit", Some("gpu_full"), Some(0)));
        assert_eq!(short(&r[0]), "the GPU limit is Off");
        let half = Facts { gpu_limit: 50, planned: 18, actual: Some(18), ..laptop() };
        assert_eq!(place(&half), Place::Partly);
        assert_eq!(short(&reasons(&half)[0]), "the GPU limit is 50%");

        // Cool & quiet on battery.
        let battery = Facts { cool_on_battery: true, planned: 25, actual: Some(25), ..laptop() };
        assert_eq!(codes(&battery), ["cool_on_battery"]);
        assert_eq!(reasons(&battery)[0].fix, Some("perf_settings"));

        // A 2 GB card: nothing of a 8B model fits.
        let small = Facts { card_can_take: Some(0), planned: 0, actual: Some(0), ..laptop() };
        let r = reasons(&small);
        assert_eq!((r[0].code, r[0].fix, r[0].number), ("too_small", None, Some(0)));
        assert!(r[0].message.contains("too small"));

        // Advanced settings set the layers.
        let advanced = Facts { advanced_layers: Some(0), planned: 0, actual: Some(0), ..laptop() };
        assert_eq!((reasons(&advanced)[0].code, reasons(&advanced)[0].fix), ("gpu_layers", Some("advanced_settings")));

        // The engine (Vulkan) didn't list the card, or ran on the processor
        // although it was asked for the card: the driver.
        let unseen = Facts { engine_sees_card: Some(false), actual: Some(0), ..laptop() };
        assert_eq!(codes(&unseen), ["engine_no_card"]);
        let ignored = Facts { actual: Some(0), ..laptop() };
        assert_eq!(codes(&ignored), ["engine_no_card"]);
        assert_eq!(reasons(&ignored)[0].fix, Some("recheck"));
        // Before the engine reported anything: the plan says where.
        let planned = Facts { actual: None, planned: 0, gpu_limit: 0, ..laptop() };
        assert_eq!(codes(&planned), ["gpu_limit"]);
    }

    /// The real settings: Cool & quiet on battery keeps 30% of the card
    /// free, which costs a model that just fits some of its layers.
    #[test]
    fn cool_and_quiet_counts_only_when_it_costs_layers() {
        use crate::catalog::GIB;
        let c = crate::catalog::Catalog::bundled();
        let m = c.model("qwen3-8b").unwrap();
        let card = |gb: u64| Hardware {
            cpu_cores: Some(8),
            cpu_threads: 16,
            ram_total: 16 * GIB,
            disk_free: 500 * GIB,
            gpus: vec![crate::hardware::Gpu { name: "GPU".into(), vendor: "NVIDIA".into(), vram_bytes: gb * GIB, integrated: false, luid: None, dxgi: None }],
            ..Default::default()
        };
        let s = PerfSettings::default();
        // Cool & quiet (on battery or chosen) uses the whole card now, as
        // Balanced does: it never costs layers, so it's never a reason.
        for (hw, mode, battery) in [(card(8), Mode::Balanced, true), (card(8), Mode::Cool, false), (card(24), Mode::Balanced, true)] {
            let mut f = Facts::default();
            setting_facts(&mut f, m, "Q4_K_M", &hw, &PerfSettings { mode, ..s.clone() }, battery);
            assert!(!f.cool_on_battery && !f.cool_mode);
        }
        // What the card takes with nothing holding it back: all on 12 GB,
        // some on 4 GB, and a few even on 2 GB (compressed cache).
        let all = m.arch.n_layer + 1;
        assert_eq!(card_can_take(m, "Q4_K_M", &card(12)), all);
        let four = card_can_take(m, "Q4_K_M", &card(4));
        assert!(four > 0 && four < all, "{four}");
        assert!(card_can_take(m, "Q4_K_M", &card(2)) > 0);
    }
}

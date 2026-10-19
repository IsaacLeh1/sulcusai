// SPDX-License-Identifier: AGPL-3.0-only
//! The model catalog and the spec filter: decides which models this PC can run.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::hardware::Hardware;

pub const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Compute buffers, CUDA/Vulkan context and other runtime memory on top of
/// weights and KV cache.
pub const RUNTIME_OVERHEAD: u64 = 700 * MIB;
/// Slowest generation speed (tokens/second) we still call usable.
pub const MIN_TPS: f64 = 4.0;
/// A GPU variant at or above this speed counts as comfortable.
const COMFORTABLE_TPS: f64 = 15.0;
/// Real-world share of theoretical memory bandwidth used per token.
const BANDWIDTH_EFFICIENCY: f64 = 0.6;
/// Disk kept free after a download.
const DISK_RESERVE: u64 = 2 * GIB;
/// The window the suggested version leaves room for: agent work (tool
/// output, documents) soon outgrows the default 8K.
const ROOMY_CTX: u32 = 16_384;
/// Below this share of its weights read per token, a model is a mixture of
/// experts, whose experts can wait in system memory.
const MOE_ACTIVE: f64 = 0.5;
/// A picture reader's work buffers, on top of its file.
pub const VISION_OVERHEAD: u64 = 300 * MIB;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalog {
    pub schema: u32,
    pub updated: String,
    pub engine: EngineSpec,
    pub models: Vec<ModelSpec>,
    pub speech: crate::speech::SpeechCatalog,
    pub voices: crate::natural::VoiceCatalog,
    pub diarization: crate::diarize::SpeakerModel,
    pub media: crate::media::MediaCatalog,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EngineSpec {
    pub name: String,
    pub license: String,
    pub build: String,
    pub assets: HashMap<String, Asset>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct License {
    pub name: String,
    pub url: String,
    pub commercial: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Arch {
    pub n_layer: u32,
    pub n_kv_heads: u32,
    pub head_dim: u32,
    pub max_ctx: u32,
    /// Share of the weights read per token (below 1.0 for mixture-of-experts).
    pub active_fraction: f64,
    /// Layers with conversation memory of their own, when layers differ:
    /// how many keep the whole window and how many only the last
    /// `swa_window` tokens (Gemma 3 4B: 5 and 29). The other layers share
    /// another layer's memory (Gemma 4's smaller models) or keep only a
    /// small fixed state (Qwen3.5's linear-attention layers). Both 0: every
    /// layer keeps the whole window.
    #[serde(default)]
    pub kv_whole: u32,
    #[serde(default)]
    pub kv_sliding: u32,
    /// The K/V width (heads × head size) of whole-window layers, when it
    /// differs from `n_kv_heads × head_dim` (Gemma 4: 1 × 512 against 8 × 256).
    #[serde(default)]
    pub kv_whole_width: u32,
    #[serde(default)]
    pub swa_window: u32,
}

/// One in how many layers keeps the whole window, for families whose other
/// layers slide and whose files don't list which (llama.cpp sets these per
/// architecture). 0: unknown, so planned as if every layer kept everything.
pub fn swa_every(architecture: &str) -> u32 {
    match architecture {
        "gemma3" => 6,
        "gemma2" | "gpt-oss" => 2,
        "cohere2" => 4,
        _ => 0,
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Variant {
    pub quant: String,
    pub quality: String,
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    /// A model split across files ("…-00001-of-00003.gguf"): every part,
    /// the first one included, in order. `file`, `url` and `sha256` are then
    /// the first part's, and `size` is all of them together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
}

/// One file of a split model.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Part {
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelSpec {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub source: String,
    pub description: String,
    pub license: License,
    pub tags: Vec<String>,
    pub params_b: f64,
    pub arch: Arch,
    pub default_ctx: u32,
    /// The model's chat template supports tool calling.
    #[serde(default)]
    pub tools: bool,
    pub variants: Vec<Variant>,
    #[serde(default)]
    pub ratings: Ratings,
    /// The image encoder (llama.cpp mmproj) for models that can see pictures.
    #[serde(default)]
    pub vision: Option<VisionFile>,
    /// Set for a model another app downloaded, used where it is.
    #[serde(default)]
    pub local: Option<LocalSource>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocalSource {
    /// The app that downloaded it, e.g. "Ollama".
    pub app: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VisionFile {
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// Rough capability guides for the Models page (from public benchmarks).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ratings {
    /// 0-100: the ranking from least to most capable.
    pub overall: u8,
    /// 0-10 by area.
    pub coding: u8,
    pub writing: u8,
    pub research: u8,
    /// 0 when the model can't use tools here.
    pub agents: u8,
    pub languages: u8,
}

impl Catalog {
    pub fn bundled() -> Catalog {
        serde_json::from_str(include_str!("../../catalog/catalog.json"))
            .expect("bundled catalog.json is valid")
    }

    pub fn model(&self, id: &str) -> Option<&ModelSpec> {
        self.models.iter().find(|m| m.id == id)
    }
}

impl ModelSpec {
    pub fn variant(&self, quant: &str) -> Option<&Variant> {
        self.variants.iter().find(|v| v.quant == quant)
    }
}

/// What this PC can give a model.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Budget {
    /// Usable dedicated VRAM on the best GPU (0 = no usable GPU).
    pub vram: u64,
    /// RAM the model may use without starving Windows and other apps.
    pub ram: u64,
    pub disk: u64,
    pub gpu_bw_gbs: f64,
    pub cpu_bw_gbs: f64,
}

impl Budget {
    pub fn from_hardware(hw: &Hardware) -> Budget {
        let gpu = hw.best_gpu();
        let vram = gpu.map_or(0, |g| {
            // Leave room for the desktop and other apps.
            let reserve = (g.vram_bytes / 10).max(512 * MIB);
            g.vram_bytes.saturating_sub(reserve)
        });
        let ram_reserve = (hw.ram_total / 4).max(4 * GIB);
        Budget {
            vram,
            ram: hw.ram_total.saturating_sub(ram_reserve),
            disk: hw.disk_free.saturating_sub(DISK_RESERVE),
            gpu_bw_gbs: gpu.map_or(0.0, |g| gpu_bandwidth_guess(g.vram_bytes)),
            cpu_bw_gbs: 50.0,
        }
    }
}

impl Budget {
    /// The budget within the performance limits (mode, Turbo, heat).
    pub fn with_limits(mut self, l: &crate::perf::Limits) -> Budget {
        self.vram = self.vram.min(l.vram_bytes);
        self.ram = self.ram.min(l.ram_bytes);
        self
    }
}

/// Context size and GPU layers within the performance limits. A RAM limit
/// too tight for the model never pushes it wholly onto the processor (the
/// hottest, slowest place for it); it keeps the layers the VRAM limit allows.
pub fn launch_within(model: &ModelSpec, quant: &str, full: &Budget, l: &crate::perf::Limits) -> (u32, u32) {
    let limited = full.with_limits(l);
    let (mut ctx, mut layers) = launch_settings(model, quant, &limited);
    if layers == 0 && limited.vram > 0 {
        (ctx, layers) = launch_settings(model, quant, &Budget { ram: full.ram, ..limited });
    }
    (ctx.min(l.max_ctx), shared(layers, l.gpu_share))
}

/// A hot graphics card (or a GPU limit): some layers move to the processor,
/// even when the whole model would fit, so the card does less of the work.
fn shared(layers: u32, gpu_share: f64) -> u32 {
    if gpu_share < 1.0 {
        (layers as f64 * gpu_share).floor() as u32
    } else {
        layers
    }
}

/// Context and GPU layers for a window the person chose: the largest up to
/// `want` (halving) that this PC still runs, never less than `auto`. The
/// card keeps `gpu_share` of the layers, as at the automatic size.
pub fn launch_at(model: &ModelSpec, quant: &str, b: &Budget, want: u32, auto: (u32, u32), gpu_share: f64) -> (u32, u32) {
    let Some(v) = model.variant(quant) else { return auto };
    let mut ctx = want.min(model.arch.max_ctx);
    while ctx > auto.0 {
        let f = fit_variant(model, v, ctx, b);
        if f.placement.is_some() {
            return (ctx, shared(f.gpu_layers, gpu_share));
        }
        ctx /= 2;
    }
    auto
}

/// Sets `bytes` aside from the budget and the limits (a picture reader
/// loaded beside the model): on the graphics card when there is one.
pub fn set_aside(b: &mut Budget, l: &mut crate::perf::Limits, bytes: u64) {
    if b.vram > 0 {
        b.vram = b.vram.saturating_sub(bytes);
        l.vram_bytes = l.vram_bytes.saturating_sub(bytes);
    } else {
        b.ram = b.ram.saturating_sub(bytes);
        l.ram_bytes = l.ram_bytes.saturating_sub(bytes);
    }
}

/// A mixture-of-experts model that doesn't fit the graphics card: rather
/// than whole layers on the processor, only the experts of the first N
/// layers wait in system memory (llama.cpp's --n-cpu-moe), so attention and
/// the shared weights all run on the card. As many bytes stay on the card
/// as with `gpu_layers` whole layers. None for other models, a model wholly
/// on the card or the processor, or one whose experts alone don't make room.
pub fn moe_offload(model: &ModelSpec, quant: &str, gpu_layers: u32) -> Option<u32> {
    let a = &model.arch;
    let all = a.n_layer + 1;
    if a.active_fraction >= MOE_ACTIVE || a.n_layer == 0 || gpu_layers == 0 || gpu_layers >= all {
        return None;
    }
    let size = model.variant(quant)?.size as f64;
    let off_card = size * (all - gpu_layers) as f64 / all as f64;
    // At least the weights a token doesn't read are experts.
    let experts_per_layer = size * (1.0 - a.active_fraction) / a.n_layer as f64;
    let n = (off_card / experts_per_layer).ceil() as u32;
    (n <= a.n_layer).then_some(n.max(1))
}

/// Rough memory bandwidth by VRAM tier, replaced by a measured benchmark
/// after install.
fn gpu_bandwidth_guess(vram: u64) -> f64 {
    match vram / GIB {
        v if v >= 20 => 700.0,
        v if v >= 14 => 500.0,
        v if v >= 10 => 380.0,
        v if v >= 6 => 250.0,
        _ => 150.0,
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Whole model on the graphics card.
    Gpu,
    /// Split between graphics card and system memory.
    Split,
    /// System memory and processor only.
    Cpu,
}

#[derive(Debug, Clone, Serialize)]
pub struct VariantFit {
    pub quant: String,
    pub placement: Option<Placement>,
    pub needed_bytes: u64,
    pub est_tps: f64,
    pub gpu_layers: u32,
    pub disk_ok: bool,
}

impl VariantFit {
    pub fn runnable(&self) -> bool {
        self.placement.is_some()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelFit {
    pub ctx: u32,
    pub variants: Vec<VariantFit>,
    pub recommended: Option<String>,
}

impl ModelFit {
    /// Shown in the catalog: some variant runs and fits on disk.
    pub fn visible(&self) -> bool {
        self.variants.iter().any(|v| v.runnable() && v.disk_ok)
    }

    pub fn variant(&self, quant: &str) -> Option<&VariantFit> {
        self.variants.iter().find(|v| v.quant == quant)
    }
}

/// f16 K and V for every layer at the given context length.
pub fn kv_cache_bytes(arch: &Arch, ctx: u32) -> u64 {
    let width = arch.n_kv_heads as u64 * arch.head_dim as u64;
    let (whole, sliding) = if arch.kv_whole + arch.kv_sliding > 0 { (arch.kv_whole, arch.kv_sliding) } else { (arch.n_layer, 0) };
    let whole_width = if arch.kv_whole_width > 0 { arch.kv_whole_width as u64 } else { width };
    // A sliding layer keeps its window plus one batch (llama.cpp: n_swa +
    // n_ubatch), never more than the whole window.
    let window = (arch.swa_window + 512).min(ctx) as u64;
    // K and V, two bytes a value.
    4 * (whole as u64 * whole_width * ctx as u64 + sliding as u64 * width * window)
}

fn tps(bytes_per_token: f64, bw_gbs: f64) -> f64 {
    if bytes_per_token <= 0.0 || bw_gbs <= 0.0 {
        return 0.0;
    }
    BANDWIDTH_EFFICIENCY * bw_gbs * 1e9 / bytes_per_token
}

/// K and V stored as q8_0 (`-ctk q8_0 -ctv q8_0`): 8.5 bits a value
/// instead of 16. Needs flash attention, which the engine turns on by itself
/// where the card supports it.
pub fn kv_cache_bytes_q8(arch: &Arch, ctx: u32) -> u64 {
    kv_cache_bytes(arch, ctx) * 17 / 32
}

/// The compressed cache when it gets more out of the graphics card: the
/// whole model on the card where the normal cache leaves layers on the
/// processor, or a 16K window where the normal cache allows only less. A
/// model that already fits roomily keeps the normal cache, a few percent
/// faster. Measured on a 12 GB RTX 5070 with Qwen2.5-Coder 14B at 8K: 47
/// layers with the normal cache wrote 35.6 tokens/s, all 49 with the
/// compressed one 49.4. Returns the window and layers to use it with.
/// `grow`: the window may grow to 16K (not when the person chose one).
pub fn with_q8_cache(model: &ModelSpec, quant: &str, b: &Budget, ctx: u32, layers: u32, grow: bool) -> Option<(u32, u32)> {
    let v = model.variant(quant)?;
    if b.vram == 0 || layers == 0 {
        return None;
    }
    let all = model.arch.n_layer + 1;
    if layers >= all {
        if !grow {
            return None;
        }
        let roomy = ROOMY_CTX.min(model.arch.max_ctx);
        return (ctx < roomy && fit_with(model, v, roomy, b, true).placement == Some(Placement::Gpu)).then_some((roomy, all));
    }
    let f = fit_with(model, v, ctx, b, true);
    match f.placement {
        Some(Placement::Gpu) => Some((ctx, all)),
        Some(Placement::Split) if f.gpu_layers > layers => Some((ctx, f.gpu_layers)),
        _ => None,
    }
}

pub fn fit_variant(model: &ModelSpec, v: &Variant, ctx: u32, b: &Budget) -> VariantFit {
    fit_with(model, v, ctx, b, false)
}

fn fit_with(model: &ModelSpec, v: &Variant, ctx: u32, b: &Budget, q8: bool) -> VariantFit {
    let kv = if q8 { kv_cache_bytes_q8(&model.arch, ctx) } else { kv_cache_bytes(&model.arch, ctx) };
    let needed = v.size + kv + RUNTIME_OVERHEAD;
    let per_token = v.size as f64 * model.arch.active_fraction;
    let all_layers = model.arch.n_layer + 1; // +1 offloads the output layer too

    let (placement, est_tps, gpu_layers) = if b.vram > 0 && needed <= b.vram {
        (Some(Placement::Gpu), tps(per_token, b.gpu_bw_gbs), all_layers)
    } else if b.vram > 0 && needed <= b.vram + b.ram {
        // KV cache and buffers stay on the GPU; weights fill what is left.
        let gpu_weights = b.vram.saturating_sub(kv + RUNTIME_OVERHEAD) as f64;
        let frac = (gpu_weights / v.size as f64).clamp(0.0, 1.0);
        let secs = per_token * frac / (BANDWIDTH_EFFICIENCY * b.gpu_bw_gbs * 1e9)
            + per_token * (1.0 - frac) / (BANDWIDTH_EFFICIENCY * b.cpu_bw_gbs * 1e9);
        let layers = (model.arch.n_layer as f64 * frac).floor() as u32;
        (Some(Placement::Split), if secs > 0.0 { 1.0 / secs } else { 0.0 }, layers)
    } else if needed <= b.ram {
        (Some(Placement::Cpu), tps(per_token, b.cpu_bw_gbs), 0)
    } else {
        (None, 0.0, 0)
    };

    let placement = placement.filter(|_| est_tps >= MIN_TPS);
    VariantFit {
        quant: v.quant.clone(),
        placement,
        needed_bytes: needed,
        est_tps: (est_tps * 10.0).round() / 10.0,
        gpu_layers,
        disk_ok: v.size <= b.disk,
    }
}

pub fn fit_model(model: &ModelSpec, b: &Budget) -> ModelFit {
    let ctx = model.default_ctx.min(model.arch.max_ctx);
    let variants: Vec<VariantFit> = model.variants.iter().map(|v| fit_variant(model, v, ctx, b)).collect();
    // Which versions still run wholly on the card with a bigger window.
    let roomy_ctx = ROOMY_CTX.min(model.arch.max_ctx).max(ctx);
    let roomy: Vec<bool> = model.variants.iter().map(|v| fit_variant(model, v, roomy_ctx, b).placement == Some(Placement::Gpu)).collect();
    let recommended = recommend(&variants, &roomy).map(|v| v.quant.clone());
    ModelFit { ctx, variants, recommended }
}

/// Context size and GPU layers to launch with. The catalog filter uses the
/// default context; at launch, a larger window is used if the model still
/// fits the same way (agent work needs room for tool output).
pub fn launch_settings(model: &ModelSpec, quant: &str, b: &Budget) -> (u32, u32) {
    let base = fit_model(model, b);
    let Some(variant) = model.variant(quant) else { return (base.ctx, 0) };
    let base_fit = fit_variant(model, variant, base.ctx, b);
    for ctx in [32_768u32, 16_384] {
        if ctx > model.arch.max_ctx || ctx <= base.ctx {
            continue;
        }
        let f = fit_variant(model, variant, ctx, b);
        if f.placement.is_some() && f.placement == base_fit.placement && f.gpu_layers >= base_fit.gpu_layers {
            return (ctx, f.gpu_layers);
        }
    }
    (base.ctx, base_fit.gpu_layers)
}

/// Catalog variants are ordered from smallest to largest (lowest to highest
/// quality). `roomy`: per variant, it still runs wholly on the graphics
/// card with a 16K window.
fn recommend<'a>(variants: &'a [VariantFit], roomy: &[bool]) -> Option<&'a VariantFit> {
    let usable: Vec<(usize, &VariantFit)> = variants.iter().enumerate().filter(|(_, v)| v.runnable() && v.disk_ok).collect();
    let on_gpu: Vec<(usize, &VariantFit)> = usable.iter().copied().filter(|(_, v)| v.placement == Some(Placement::Gpu)).collect();
    if !on_gpu.is_empty() {
        // Highest quality that is still comfortable and leaves room for a
        // 16K window (the biggest that fits 8K gets only 8K: the window is
        // worked out after the version), else the highest comfortable, else
        // the fastest on GPU.
        let comfortable = |v: &VariantFit| v.est_tps >= COMFORTABLE_TPS;
        return on_gpu
            .iter()
            .rev()
            .find(|(i, v)| comfortable(v) && roomy.get(*i) == Some(&true))
            .or_else(|| on_gpu.iter().rev().find(|(_, v)| comfortable(v)))
            .or(on_gpu.first())
            .map(|(_, v)| *v);
    }
    // Off the GPU every extra byte costs speed, so take the fastest.
    usable
        .into_iter()
        .map(|(_, v)| v)
        .max_by(|a, b| a.est_tps.partial_cmp(&b.est_tps).unwrap_or(std::cmp::Ordering::Equal))
}

/// The model to suggest on this PC: the most capable general assistant
/// (chat and tools, by its overall rating) whose best version runs
/// comfortably here. With a graphics card it must fit on the card entirely:
/// a split model can be quick to write but is slow to read long prompts and
/// keeps the processor busy. Without one (or when nothing fits), the most
/// capable that's comfortable anywhere; on a slow PC, the fastest.
pub fn best_for(models: &[ModelSpec], b: &Budget) -> Option<String> {
    let picks: Vec<(&ModelSpec, VariantFit)> = models
        .iter()
        .filter(|m| m.tools && m.tags.iter().any(|t| t == "chat"))
        .filter_map(|m| {
            let fit = fit_model(m, b);
            let v = fit.variant(fit.recommended.as_deref()?)?.clone();
            (v.runnable() && v.disk_ok).then_some((m, v))
        })
        .collect();
    let best = |on_gpu: bool| {
        picks
            .iter()
            .filter(|(_, v)| v.est_tps >= COMFORTABLE_TPS && (!on_gpu || v.placement == Some(Placement::Gpu)))
            .max_by_key(|(m, _)| m.ratings.overall)
            .map(|(m, _)| m.id.clone())
    };
    (if b.vram > 0 { best(true) } else { None })
        .or_else(|| best(false))
        .or_else(|| picks.iter().max_by(|a, b| a.1.est_tps.partial_cmp(&b.1.est_tps).unwrap_or(std::cmp::Ordering::Equal)).map(|(m, _)| m.id.clone()))
}

#[derive(Debug, Clone, Serialize)]
pub struct UpgradeHint {
    pub extra_ram_gb: u64,
    pub unlocks: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hints {
    /// Models hidden because this PC can't run them.
    pub hidden: usize,
    pub ram: Vec<UpgradeHint>,
    /// Models that would run but don't fit on the disk.
    pub disk_blocked: usize,
    pub disk_needed_bytes: u64,
}

pub fn hints(catalog: &Catalog, b: &Budget, installed: &dyn Fn(&str) -> bool) -> Hints {
    let mut hidden = 0;
    let mut disk_blocked = 0;
    let mut disk_needed = u64::MAX;
    let mut cant_run: Vec<&ModelSpec> = Vec::new();

    for m in &catalog.models {
        if installed(&m.id) {
            continue;
        }
        let fit = fit_model(m, b);
        if fit.visible() {
            continue;
        }
        hidden += 1;
        let runnable: Vec<&VariantFit> = fit.variants.iter().filter(|v| v.runnable()).collect();
        if runnable.is_empty() {
            cant_run.push(m);
        } else {
            disk_blocked += 1;
            for v in runnable {
                let size = m.variant(&v.quant).map_or(0, |s| s.size);
                disk_needed = disk_needed.min(size.saturating_sub(b.disk));
            }
        }
    }

    let mut ram = Vec::new();
    let mut already = 0;
    for extra in [8u64, 16, 32] {
        let bigger = Budget { ram: b.ram + extra * GIB, ..*b };
        let unlocks = cant_run
            .iter()
            .filter(|m| fit_model(m, &bigger).variants.iter().any(|v| v.runnable()))
            .count();
        if unlocks > already {
            ram.push(UpgradeHint { extra_ram_gb: extra, unlocks });
            already = unlocks;
        }
    }

    Hints {
        hidden,
        ram,
        disk_blocked,
        disk_needed_bytes: if disk_blocked > 0 { disk_needed } else { 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(vram_gb: u64, ram_gb: u64) -> Budget {
        Budget {
            vram: vram_gb * GIB,
            ram: ram_gb * GIB,
            disk: 500 * GIB,
            gpu_bw_gbs: if vram_gb > 0 { 380.0 } else { 0.0 },
            cpu_bw_gbs: 50.0,
        }
    }

    fn catalog() -> Catalog {
        Catalog::bundled()
    }

    /// What a PC with this much graphics memory (0: none) and system memory gets.
    fn pc(vram_gb: u64, ram_gb: u64) -> Budget {
        let hw = Hardware {
            ram_total: ram_gb * GIB,
            disk_free: 2_000 * GIB,
            gpus: if vram_gb > 0 { vec![crate::hardware::Gpu { name: "GPU".into(), vendor: "NVIDIA".into(), vram_bytes: vram_gb * GIB, integrated: false, luid: None }] } else { vec![] },
            ..Default::default()
        };
        Budget::from_hardware(&hw)
    }

    /// A bigger graphics card gets a more capable suggestion: a 12 GB laptop
    /// (his) and a 16 GB card (his friend's) differ, and 24 GB gets the most.
    #[test]
    fn the_suggestion_grows_with_the_graphics_card() {
        let c = catalog();
        let best = |vram, ram| best_for(&c.models, &pc(vram, ram)).unwrap();
        assert_eq!(best(12, 32), "qwen3-14b");
        assert_eq!(best(16, 32), "gpt-oss-20b");
        assert_eq!(best(24, 64), "qwen3-32b");
        assert_eq!(best(8, 16), "qwen3-8b");
        // No graphics card: something comfortable on the processor, never a big model.
        let cpu = best(0, 16);
        let spec = c.model(&cpu).unwrap();
        assert!(spec.params_b <= 8.0, "{cpu}");
        // Coding-only and picture-only models aren't suggested as the assistant.
        for vram in [8, 12, 16, 24, 48] {
            let id = best(vram, 64);
            assert!(!id.contains("coder"), "{vram} GB: {id}");
        }
    }

    /// A bigger window chosen by the person: as big as still runs, layers
    /// moving off the graphics card for the room; never below automatic.
    #[test]
    fn a_chosen_window_is_as_big_as_still_runs() {
        let c = catalog();
        let m = c.model("qwen3-8b").unwrap();
        let q = "Q4_K_M";
        let b = budget(12, 24);
        let auto = launch_settings(m, q, &b);
        let (ctx, layers) = launch_at(m, q, &b, 32_768, auto, 1.0);
        assert_eq!(ctx, 32_768.min(m.arch.max_ctx));
        assert!(layers > 0);
        let (huge, _) = launch_at(m, q, &b, 1 << 20, auto, 1.0);
        assert!(huge <= m.arch.max_ctx, "never past what the model supports");
        // A hot card keeps fewer layers at a chosen window too.
        let (bigger, all) = launch_at(m, q, &b, 65_536, auto, 1.0);
        assert!(bigger > auto.0);
        assert_eq!(launch_at(m, q, &b, 65_536, auto, 0.5), (bigger, all / 2));
        // Little memory: the request shrinks until it runs, never below automatic.
        let small = budget(0, 8);
        let auto_small = launch_settings(m, q, &small);
        let (ctx_small, _) = launch_at(m, q, &small, 131_072, auto_small, 1.0);
        assert!(ctx_small >= auto_small.0);
        assert!(ctx_small == auto_small.0 || fit_variant(m, m.variant(q).unwrap(), ctx_small, &small).placement.is_some());
    }

    /// The suggested version leaves room for a 16K window when one does: on
    /// a 12 GB card Qwen3 8B's biggest version fits only 8K. When none does,
    /// the choice is as before.
    #[test]
    fn the_suggested_version_leaves_room_for_a_bigger_window() {
        let c = catalog();
        let m = c.model("qwen3-8b").unwrap();
        let b = pc(12, 32);
        let fit = fit_model(m, &b);
        assert_eq!(fit.variant("Q8_0").unwrap().placement, Some(Placement::Gpu), "the biggest still fits at 8K");
        assert_eq!(launch_settings(m, "Q8_0", &b).0, 8192, "but gets only 8K");
        let rec = fit.recommended.as_deref().unwrap();
        assert_eq!(rec, "Q6_K");
        assert!(launch_settings(m, rec, &b).0 >= 16_384);
        let big = c.model("qwen3-14b").unwrap();
        assert_eq!(fit_model(big, &b).recommended.as_deref(), Some("Q4_K_M"));
        // A small model gets its best version with room to spare.
        let small = c.model("qwen3-4b-2507").unwrap();
        assert_eq!(fit_model(small, &b).recommended.as_deref(), Some("Q8_0"));
    }

    /// A picture reader takes room of its own: a vision model that just fit
    /// the card with its window gets a smaller window (or fewer layers) when
    /// the reader is loaded beside it.
    #[test]
    fn the_picture_reader_counts_in_the_plan() {
        let c = catalog();
        let m = c.model("qwen3-vl-8b").unwrap();
        let reader = m.vision.as_ref().unwrap().size + VISION_OVERHEAD;
        let full = pc(12, 32);
        let l = crate::perf::Limits {
            mode: crate::perf::Mode::Balanced,
            on_battery: false,
            threads: 8,
            vram_bytes: full.vram,
            ram_bytes: full.ram,
            low_priority: false,
            idle_unload_mins: 10,
            max_ctx: 32_768,
            gpu_temp_limit: 90.0,
            cpu_temp_limit: 95.0,
            heat_level: 0,
            gpu_share: 1.0,
        };
        let without = launch_within(m, "Q8_0", &full, &l);
        let (mut b, mut lw) = (full, l);
        set_aside(&mut b, &mut lw, reader);
        assert_eq!(b.vram, full.vram - reader);
        let with = launch_within(m, "Q8_0", &b, &lw);
        assert!(with.0 < without.0 || with.1 < without.1, "{without:?} vs {with:?}");
        // Without a graphics card it comes out of system memory.
        let (mut cpu, mut lc) = (budget(0, 16), crate::perf::Limits { vram_bytes: 0, ram_bytes: 16 * GIB, ..l });
        set_aside(&mut cpu, &mut lc, reader);
        assert_eq!((cpu.ram, lc.ram_bytes), (16 * GIB - reader, 16 * GIB - reader));
    }

    /// A mixture-of-experts model too big for the card keeps its experts,
    /// not whole layers, in system memory, as many bytes as the split would.
    #[test]
    fn experts_wait_in_memory_rather_than_whole_layers() {
        let c = catalog();
        let moe = c.model("qwen3-30b-a3b-2507").unwrap();
        let b = budget(12, 32);
        let fit = fit_variant(moe, moe.variant("Q4_K_M").unwrap(), 8192, &b);
        assert_eq!(fit.placement, Some(Placement::Split));
        let n = moe_offload(moe, "Q4_K_M", fit.gpu_layers).expect("experts in memory");
        assert!(n > 0 && n < moe.arch.n_layer, "{n}");
        // The experts of n layers make at least as much room as the split.
        let size = moe.variant("Q4_K_M").unwrap().size as f64;
        let all = (moe.arch.n_layer + 1) as f64;
        let freed = n as f64 * size * (1.0 - moe.arch.active_fraction) / moe.arch.n_layer as f64;
        assert!(freed >= size * (all - fit.gpu_layers as f64) / all);
        // Wholly on the card, on the processor alone, or not a mixture of experts: as before.
        assert_eq!(moe_offload(moe, "Q4_K_M", moe.arch.n_layer + 1), None);
        assert_eq!(moe_offload(moe, "Q4_K_M", 0), None);
        let dense = c.model("qwen3-14b").unwrap();
        assert_eq!(moe_offload(dense, "Q4_K_M", 20), None);
        assert!(moe_offload(c.model("gpt-oss-20b").unwrap(), "Q4_K_M", 10).is_some());
    }

    /// The compressed cache when it puts the whole model on the card, or
    /// gives a 16K window the normal cache can't; not otherwise.
    #[test]
    fn sliding_window_layers_keep_only_their_window() {
        // Gemma 3 4B at 16K, as llama.cpp b11450 allocated it: 5 layers of
        // 16384 cells (320 MiB) and 29 of 1536 (174 MiB).
        let c = Catalog::bundled();
        let gemma = &c.model("gemma-3-4b").unwrap().arch;
        assert_eq!(kv_cache_bytes(gemma, 16384), (320 + 174) * 1024 * 1024);
        let full = Arch { swa_window: 0, kv_whole: 0, kv_sliding: 0, ..gemma.clone() };
        assert!(kv_cache_bytes(&full, 16384) > 4 * kv_cache_bytes(gemma, 16384), "the old estimate was 4.6 times too big");
        // A window smaller than the slide keeps everything everywhere.
        assert_eq!(kv_cache_bytes(gemma, 1024), kv_cache_bytes(&full, 1024));
    }

    #[test]
    fn the_compressed_cache_puts_the_whole_model_on_the_card() {
        let c = catalog();
        let m = c.model("qwen3-14b").unwrap();
        let v = m.variant("Q4_K_M").unwrap();
        let all = m.arch.n_layer + 1;
        let card = |bytes: u64| Budget { vram: bytes + RUNTIME_OVERHEAD + 64 * MIB, ..budget(12, 32) };
        // Room for the model and a compressed 8K cache, not a normal one
        // (Qwen2.5-Coder 14B on his 12 GB card: 47 of 49 layers).
        let tight = card(v.size + kv_cache_bytes_q8(&m.arch, 8192));
        let f = fit_variant(m, v, 8192, &tight);
        assert_eq!(f.placement, Some(Placement::Split));
        assert_eq!(with_q8_cache(m, "Q4_K_M", &tight, 8192, f.gpu_layers, true), Some((8192, all)));
        // A 16K window he chose: kept, with more of the model on the card
        // (his 14B: 39 layers with the normal cache).
        let chosen = fit_variant(m, v, 16_384, &tight);
        let (c, more) = with_q8_cache(m, "Q4_K_M", &tight, 16_384, chosen.gpu_layers, false).expect("more layers");
        assert!(c == 16_384 && more > chosen.gpu_layers, "{} -> {more}", chosen.gpu_layers);
        // Whole on the card at 8K, but no room for 16K either way: as it is.
        let fits = card(v.size + kv_cache_bytes(&m.arch, 8192));
        assert_eq!(fit_variant(m, v, 8192, &fits).placement, Some(Placement::Gpu));
        assert_eq!(with_q8_cache(m, "Q4_K_M", &fits, 8192, all, true), None);
        // Room for a compressed 16K window: 16K.
        let wide = card(v.size + kv_cache_bytes_q8(&m.arch, 16_384));
        assert_eq!(with_q8_cache(m, "Q4_K_M", &wide, 8192, all, true), Some((16_384, all)));
        assert_eq!(with_q8_cache(m, "Q4_K_M", &wide, 8192, all, false), None, "a window he chose stays as it is");
        assert_eq!(with_q8_cache(m, "Q4_K_M", &wide, 16_384, all, true), None, "already roomy: the faster normal cache");
        // No card: nothing to gain.
        assert_eq!(with_q8_cache(m, "Q4_K_M", &budget(0, 32), 8192, 0, true), None);
        assert_eq!(kv_cache_bytes_q8(&m.arch, 8192) * 32, kv_cache_bytes(&m.arch, 8192) * 17);
    }

    #[test]
    fn bundled_catalog_is_consistent() {
        let c = catalog();
        assert!(!c.models.is_empty());
        for m in &c.models {
            assert!(!m.variants.is_empty(), "{} has no variants", m.id);
            for v in &m.variants {
                assert_eq!(v.sha256.len(), 64, "{} {} hash", m.id, v.quant);
                assert!(v.url.starts_with("https://huggingface.co/"));
            }
            // Variants must go from smallest to largest for `recommend`.
            assert!(m.variants.windows(2).all(|w| w[0].size <= w[1].size), "{} order", m.id);
        }
        assert!(c.engine.assets.contains_key("vulkan-x64"));
        assert!(c.engine.assets.contains_key("cpu-x64"));
    }

    #[test]
    fn kv_cache_matches_hand_calculation() {
        let c = catalog();
        let qwen8 = c.model("qwen3-8b").unwrap();
        // 2 (K,V) * 36 layers * 8 heads * 128 dim * 8192 ctx * 2 bytes
        assert_eq!(kv_cache_bytes(&qwen8.arch, 8192), 1_207_959_552);
    }

    #[test]
    fn small_model_fits_fully_on_a_12gb_gpu() {
        let c = catalog();
        let m = c.model("qwen3-4b-2507").unwrap();
        let fit = fit_model(m, &budget(11, 24));
        assert!(fit.variants.iter().all(|v| v.placement == Some(Placement::Gpu)));
        assert!(fit.recommended.is_some());
    }

    #[test]
    fn big_model_splits_when_vram_is_short() {
        let c = catalog();
        let m = c.model("qwen3-14b").unwrap();
        let fit = fit_model(m, &budget(8, 24));
        let q4 = fit.variant("Q4_K_M").unwrap();
        assert_eq!(q4.placement, Some(Placement::Split));
        assert!(q4.gpu_layers > 0 && q4.gpu_layers < m.arch.n_layer);
    }

    #[test]
    fn model_is_hidden_when_nothing_fits() {
        let c = catalog();
        let m = c.model("qwen3-14b").unwrap();
        // 4 GB of RAM and no GPU can't hold a 9 GB model.
        assert!(!fit_model(m, &budget(0, 4)).visible());
    }

    #[test]
    fn cpu_only_pc_still_gets_small_models() {
        let c = catalog();
        let m = c.model("qwen3-1.7b").unwrap();
        let fit = fit_model(m, &budget(0, 12));
        assert!(fit.visible());
        let rec = fit.variant(fit.recommended.as_deref().unwrap()).unwrap();
        assert_eq!(rec.placement, Some(Placement::Cpu));
        // Without a GPU the fastest (smallest) variant is recommended.
        assert_eq!(rec.quant, "Q4_K_M");
    }

    #[test]
    fn too_slow_counts_as_not_runnable() {
        let c = catalog();
        let m = c.model("qwen3-14b").unwrap();
        // Fits in 32 GB RAM, but ~3 tokens/sec on CPU is below MIN_TPS.
        let fit = fit_model(m, &budget(0, 32));
        assert!(fit.variant("Q8_0").unwrap().placement.is_none());
    }

    #[test]
    fn launch_uses_a_bigger_window_only_when_it_still_fits_the_same_way() {
        let c = catalog();
        let small = c.model("qwen3-4b-2507").unwrap();
        assert_eq!(launch_settings(small, "Q4_K_M", &budget(11, 24)).0, 32_768);
        // Already split between GPU and RAM: don't push more off the GPU.
        let big = c.model("qwen3-14b").unwrap();
        let (ctx, _) = launch_settings(big, "Q4_K_M", &budget(8, 24));
        assert_eq!(ctx, 8192);
        // Never beyond what the model supports.
        let coder = c.model("qwen2.5-coder-7b").unwrap();
        assert!(launch_settings(coder, "Q4_K_M", &budget(20, 64)).0 <= coder.arch.max_ctx);
    }

    #[test]
    fn tool_capable_models_are_marked() {
        let c = catalog();
        assert!(c.model("qwen3-8b").unwrap().tools);
        assert!(!c.model("gemma-3-4b").unwrap().tools);
    }

    #[test]
    fn disk_shortage_hides_but_is_reported() {
        let c = catalog();
        let mut b = budget(11, 24);
        b.disk = GIB; // nothing fits on disk
        let h = hints(&c, &b, &|_| false);
        assert_eq!(h.hidden, c.models.len());
        assert!(h.disk_blocked > 0);
        assert!(h.disk_needed_bytes > 0);
    }

    #[test]
    fn ram_upgrade_hint_counts_unlocked_models() {
        let c = catalog();
        let h = hints(&c, &budget(0, 4), &|_| false);
        assert!(h.hidden > 0);
        assert!(h.ram.iter().any(|r| r.unlocks > 0));
    }
}

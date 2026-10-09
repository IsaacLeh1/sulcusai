// SPDX-License-Identifier: AGPL-3.0-only
//! The model catalog and the spec filter: decides which models this PC can run.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::hardware::Hardware;

pub const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Compute buffers, CUDA/Vulkan context and other runtime memory on top of
/// weights and KV cache.
const RUNTIME_OVERHEAD: u64 = 700 * MIB;
/// Slowest generation speed (tokens/second) we still call usable.
pub const MIN_TPS: f64 = 4.0;
/// A GPU variant at or above this speed counts as comfortable.
const COMFORTABLE_TPS: f64 = 15.0;
/// Real-world share of theoretical memory bandwidth used per token.
const BANDWIDTH_EFFICIENCY: f64 = 0.6;
/// Disk kept free after a download.
const DISK_RESERVE: u64 = 2 * GIB;

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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Variant {
    pub quant: String,
    pub quality: String,
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
    // A hot graphics card: move some layers to the processor, even when
    // the whole model would fit, so the card does less of the work.
    if l.gpu_share < 1.0 {
        layers = (layers as f64 * l.gpu_share).floor() as u32;
    }
    (ctx.min(l.max_ctx), layers)
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
    2 * arch.n_layer as u64 * arch.n_kv_heads as u64 * arch.head_dim as u64 * ctx as u64 * 2
}

fn tps(bytes_per_token: f64, bw_gbs: f64) -> f64 {
    if bytes_per_token <= 0.0 || bw_gbs <= 0.0 {
        return 0.0;
    }
    BANDWIDTH_EFFICIENCY * bw_gbs * 1e9 / bytes_per_token
}

pub fn fit_variant(model: &ModelSpec, v: &Variant, ctx: u32, b: &Budget) -> VariantFit {
    let kv = kv_cache_bytes(&model.arch, ctx);
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
    let recommended = recommend(&variants).map(|v| v.quant.clone());
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
/// quality).
fn recommend(variants: &[VariantFit]) -> Option<&VariantFit> {
    let usable: Vec<&VariantFit> = variants.iter().filter(|v| v.runnable() && v.disk_ok).collect();
    let on_gpu: Vec<&VariantFit> = usable.iter().copied().filter(|v| v.placement == Some(Placement::Gpu)).collect();
    if !on_gpu.is_empty() {
        // Highest quality that is still comfortable, else the fastest on GPU.
        return on_gpu
            .iter()
            .rev()
            .find(|v| v.est_tps >= COMFORTABLE_TPS)
            .or(on_gpu.first())
            .copied();
    }
    // Off the GPU every extra byte costs speed, so take the fastest.
    usable
        .into_iter()
        .max_by(|a, b| a.est_tps.partial_cmp(&b.est_tps).unwrap_or(std::cmp::Ordering::Equal))
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

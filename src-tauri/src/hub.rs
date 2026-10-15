// SPDX-License-Identifier: AGPL-3.0-only
//! Hugging Face: searching its GGUF chat models, showing only the files this
//! PC can run, and installing one through the normal install (progress,
//! checksum, picture reader, speed test). Models that need an account
//! (gated) and models split across several files are left out.
//!
//! Before a download, the memory a file needs while running is estimated
//! from its size and parameter count (Hugging Face doesn't list the layer
//! layout). Once it's downloaded, the file's own header replaces the
//! estimate (`refine`).

use std::path::Path;
use std::sync::Arc;

use futures_util::{stream, StreamExt};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::catalog::{self, Arch, Budget, License, ModelSpec, Placement, Ratings, Variant, VisionFile};
use crate::{db, gguf, local, net, AppState};

const API: &str = "https://huggingface.co/api/models";
const SITE: &str = "https://huggingface.co";
const KEY: &str = "hub_models";
/// Results looked at per search (each needs one more request for its files).
const PAGE: usize = 24;

/// Models installed (or being installed) from Hugging Face.
pub fn list(conn: &Connection) -> Vec<ModelSpec> {
    db::get(conn, KEY).unwrap_or_default()
}

fn remember(conn: &Connection, spec: &ModelSpec) -> Result<(), String> {
    let mut all = list(conn);
    all.retain(|m| m.id != spec.id);
    all.push(spec.clone());
    db::set(conn, KEY, &all)
}

/// Removed from the list (the install deleted its files).
pub fn forget(conn: &Connection, id: &str) -> Result<(), String> {
    let mut all = list(conn);
    all.retain(|m| m.id != id);
    db::set(conn, KEY, &all)
}

#[derive(Debug, Serialize)]
pub struct HubFile {
    /// The file's path in the repository.
    pub file: String,
    /// Memory it needs while running (estimated), for a warning.
    pub needed: u64,
    /// The id it installs as (to show its progress, or that it's installed).
    pub model_id: String,
    pub quant: String,
    pub quality: String,
    pub size: u64,
    pub placement: Option<Placement>,
    pub est_tps: f64,
}

#[derive(Debug, Serialize)]
pub struct HubModel {
    /// "owner/name" on Hugging Face.
    pub id: String,
    pub name: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    pub params_b: Option<f64>,
    pub updated: Option<String>,
    /// It comes with a picture reader (mmproj).
    pub vision: bool,
    pub url: String,
    /// Only the files this PC can run, smallest first.
    pub files: Vec<HubFile>,
    /// The file that suits this PC best.
    pub recommended: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HubResults {
    pub models: Vec<HubModel>,
    /// Results left out because no file of theirs runs on this PC.
    pub hidden: usize,
}

/// A file in a repository, with its checksum.
struct RemoteFile {
    path: String,
    size: u64,
    sha256: String,
}

/// A repository's details: its files and what Hugging Face read from its GGUF.
struct Repo {
    info: Value,
    files: Vec<RemoteFile>,
}

async fn repo(client: &reqwest::Client, id: &str) -> Result<Repo, String> {
    let info: Value = client
        .get(format!("{API}/{id}?blobs=true"))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Hugging Face answered: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let files = info["siblings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(RemoteFile {
                path: s["rfilename"].as_str()?.to_string(),
                size: s["lfs"]["size"].as_u64().or_else(|| s["size"].as_u64())?,
                sha256: s["lfs"]["sha256"].as_str()?.to_string(),
            })
        })
        .collect();
    Ok(Repo { info, files })
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_projector(path: &str) -> bool {
    base_name(path).to_lowercase().contains("mmproj")
}

/// "…-00001-of-00003.gguf": part of a model split across files.
fn is_split(path: &str) -> bool {
    let stem = base_name(path).trim_end_matches(".gguf");
    let mut it = stem.rsplit('-');
    let digits = |s: &str| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit());
    matches!((it.next(), it.next(), it.next()), (Some(t), Some("of"), Some(p)) if digits(t) && digits(p))
}

/// Helper files some repositories keep beside the model: draft models and
/// prediction heads for faster decoding, importance matrices.
fn is_helper(path: &str) -> bool {
    let name = base_name(path).to_lowercase();
    ["eagle", "draft", "mtp", "dflash", "speculator", "imatrix"].iter().any(|w| name.contains(w))
}

/// The model files a repository offers: single-file GGUFs, not picture
/// readers or helpers, and not too small to hold the whole model (real
/// quantizations keep at least ~1.5 bits per weight).
fn model_files(repo: &Repo) -> impl Iterator<Item = &RemoteFile> {
    let params = repo.info["gguf"]["total"].as_f64().unwrap_or(0.0);
    let smallest = (params * 1.5 / 8.0) as u64;
    repo.files.iter().filter(move |f| {
        f.path.to_lowercase().ends_with(".gguf") && !is_projector(&f.path) && !is_split(&f.path) && !is_helper(&f.path) && f.size >= smallest
    })
}

/// The quantization named in a file name ("…-Q4_K_M.gguf", "…-UD-Q4_K_XL.gguf").
pub fn quant_of(path: &str) -> String {
    const KNOWN: [&str; 48] = [
        "IQ1_S", "IQ1_M", "IQ2_XXS", "IQ2_XS", "IQ2_S", "IQ2_M", "IQ3_XXS", "IQ3_XS", "IQ3_S", "IQ3_M", "IQ4_XS", "IQ4_NL", "Q2_K_XL", "Q2_K_L", "Q2_K_S", "Q2_K",
        "Q3_K_XL", "Q3_K_L", "Q3_K_M", "Q3_K_S", "Q4_K_XL", "Q4_K_L", "Q4_K_M", "Q4_K_S", "Q4_0", "Q4_1", "Q5_K_XL", "Q5_K_L", "Q5_K_M", "Q5_K_S", "Q5_0", "Q5_1",
        "Q6_K_XL", "Q6_K_L", "Q6_K", "Q8_K_XL", "Q8_0", "MXFP4_MOE", "MXFP4", "BF16", "F16", "F32", "TQ1_0", "TQ2_0", "FP16", "FP32", "Q4_K", "Q5_K",
    ];
    let up = base_name(path).trim_end_matches(".gguf").to_uppercase();
    let bounded = |i: usize, len: usize| {
        let before = up[..i].chars().last().is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = up[i + len..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric());
        before && after
    };
    KNOWN
        .iter()
        .find(|q| up.match_indices(*q).any(|(i, _)| bounded(i, q.len())))
        .map(|q| match *q {
            "FP16" => "F16",
            "FP32" => "F32",
            q => q,
        })
        .unwrap_or("GGUF")
        .to_string()
}

/// The app's quality word, for quantization names the catalog doesn't use.
fn quality_of(quant: &str) -> String {
    let near = match quant {
        q if q.starts_with("Q8") => "Q8_0",
        q if q.starts_with("Q6") => "Q6_K",
        q if q.starts_with("Q5") => "Q5_K_M",
        q if q.starts_with("Q4") || q.starts_with("IQ4") || q.starts_with("MXFP4") => "Q4_K_M",
        q if q.starts_with("Q3") || q.starts_with("IQ3") => "Q3_K_M",
        q => q,
    };
    gguf::quality(near).to_string()
}

/// "30B-A3B": billions of parameters used per token, for mixture-of-experts.
fn active_b(name: &str) -> Option<f64> {
    let up = name.to_uppercase();
    up.split(|c: char| !c.is_ascii_alphanumeric() && c != '.').find_map(|w| {
        let n = w.strip_prefix('A')?.strip_suffix('B')?;
        n.parse::<f64>().ok().filter(|v| *v > 0.0)
    })
}

/// The layout a model probably has, from its size: generous, so a model
/// shown as runnable still fits once its real header is read.
fn estimate_arch(params_b: f64, architecture: &str, repo_id: &str, ctx_len: u32) -> Arch {
    let n_layer = match params_b {
        p if p < 1.5 => 28,
        p if p < 10.0 => 36,
        p if p < 20.0 => 48,
        p if p < 40.0 => 64,
        _ => 80,
    };
    let active = if architecture == "gpt-oss" { Some(if params_b < 50.0 { 3.6 } else { 5.1 }) } else { active_b(repo_id) };
    let active_fraction = match active {
        Some(a) if params_b > 0.0 && a < params_b => 0.1 + 0.9 * a / params_b,
        _ => 1.0,
    };
    Arch { n_layer, n_kv_heads: 8, head_dim: 128, max_ctx: ctx_len.clamp(2048, 131_072), active_fraction }
}

/// A short name from the repository's: "unsloth/gpt-oss-20b-GGUF" → "gpt-oss-20b".
fn display_name(id: &str) -> String {
    let name = id.rsplit('/').next().unwrap_or(id);
    let trimmed = ["-GGUF", "_GGUF", "-gguf", "_gguf", ".GGUF", "-Gguf"].iter().find_map(|s| name.strip_suffix(s)).unwrap_or(name);
    trimmed.to_string()
}

/// A spec for fitting (`quant` set to each file's path, so the recommended
/// one names a file) or for installing.
fn spec_for(repo_id: &str, info: &Value, variants: Vec<Variant>, vision: Option<VisionFile>, id: String) -> ModelSpec {
    let g = &info["gguf"];
    let size_guess = variants.first().map_or(0, |v| v.size) as f64 * 8.0 / 4.8 / 1e9;
    let params_b = g["total"].as_f64().map(|t| t / 1e9).filter(|p| *p > 0.0).unwrap_or(size_guess);
    let ctx_len = g["context_length"].as_u64().unwrap_or(8192) as u32;
    let arch = estimate_arch(params_b, g["architecture"].as_str().unwrap_or(""), repo_id, ctx_len);
    let source = format!("{SITE}/{repo_id}");
    let license = info["cardData"]["license"].as_str().map(str::to_string);
    ModelSpec {
        id,
        name: display_name(repo_id),
        publisher: info["author"].as_str().map(str::to_string).unwrap_or_else(|| repo_id.split('/').next().unwrap_or("").to_string()),
        source: source.clone(),
        description: format!("From Hugging Face ({repo_id}). It's downloaded to this PC; removing it deletes the file."),
        license: License {
            name: license.unwrap_or_else(|| "See the model's page".into()),
            url: source,
            commercial: false,
            note: Some("Check the license on the model's page before using it for work.".into()),
        },
        tags: if vision.is_some() { vec!["vision".into()] } else { Vec::new() },
        params_b: (params_b * 10.0).round() / 10.0,
        default_ctx: 8192.min(arch.max_ctx),
        arch,
        tools: g["chat_template"].as_str().is_some_and(|t| t.contains("tools")),
        variants,
        ratings: Ratings::default(),
        vision,
        local: None,
    }
}

/// What a search shows for one repository: the files this PC can run (or,
/// with `all`, every file, the ones it can't run without a placement).
fn describe(m: &Value, repo: &Repo, budget: &Budget, all: bool) -> HubModel {
    let id = m["id"].as_str().unwrap_or_default().to_string();
    let mut files: Vec<&RemoteFile> = model_files(repo).filter(|f| f.size <= budget.disk).collect();
    files.sort_by_key(|f| f.size);
    let variants: Vec<Variant> = files
        .iter()
        .map(|f| Variant { quant: f.path.clone(), quality: String::new(), file: f.path.clone(), url: String::new(), size: f.size, sha256: String::new() })
        .collect();
    let spec = spec_for(&id, &repo.info, variants, None, String::new());
    let fit = catalog::fit_model(&spec, budget);
    let files: Vec<HubFile> = files
        .iter()
        .zip(&fit.variants)
        .filter(|(_, v)| all || v.runnable())
        .map(|(f, v)| {
            let quant = quant_of(&f.path);
            HubFile { file: f.path.clone(), needed: v.needed_bytes, model_id: model_id(&id, &f.path), quality: quality_of(&quant), quant, size: f.size, placement: v.placement, est_tps: v.est_tps }
        })
        .collect();
    HubModel {
        name: display_name(&id),
        author: id.split('/').next().unwrap_or("").to_string(),
        downloads: m["downloads"].as_u64().unwrap_or(0),
        likes: m["likes"].as_u64().unwrap_or(0),
        params_b: m["gguf"]["total"].as_f64().map(|t| (t / 1e8).round() / 10.0),
        updated: m["lastModified"].as_str().map(|d| d.chars().take(10).collect()),
        vision: repo.files.iter().any(|f| is_projector(&f.path)),
        url: format!("{SITE}/{id}"),
        recommended: fit.recommended.filter(|r| files.iter().any(|f| &f.file == r)),
        files,
        id,
    }
}

/// The id a file installs as: the same for the same file, every time.
fn model_id(repo_id: &str, file: &str) -> String {
    format!("hf-{}", &hex::encode(Sha256::digest(format!("{repo_id}/{file}").as_bytes()))[..12])
}

fn client(state: &AppState) -> Result<reqwest::Client, String> {
    net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)
}

/// Searches Hugging Face's GGUF chat models (`sort`: "downloads", "trending"
/// or "newest"); an empty query lists the most popular.
pub async fn search(state: &Arc<AppState>, query: &str, sort: &str, all: bool) -> Result<HubResults, String> {
    let client = client(state)?;
    let mut url = reqwest::Url::parse(API).map_err(|e| e.to_string())?;
    {
        let mut q = url.query_pairs_mut();
        if !query.trim().is_empty() {
            q.append_pair("search", query.trim());
        }
        q.append_pair("filter", "gguf");
        q.append_pair("sort", match sort {
            "trending" => "trendingScore",
            "newest" => "lastModified",
            _ => "downloads",
        });
        q.append_pair("direction", "-1");
        q.append_pair("limit", &PAGE.to_string());
        for e in ["gguf", "downloads", "likes", "lastModified", "pipeline_tag", "gated"] {
            q.append_pair("expand[]", e);
        }
    }
    state.log("network", "Searched Hugging Face for models (huggingface.co)");
    let found: Vec<Value> = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Hugging Face answered: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    // Chat models that download without an account.
    let chat = |m: &Value| matches!(m["pipeline_tag"].as_str(), None | Some("text-generation" | "image-text-to-text"));
    let open = |m: &Value| m["gated"].is_null() || m["gated"] == false;
    let candidates: Vec<Value> = found.into_iter().filter(|m| chat(m) && open(m) && m["id"].is_string()).collect();
    let budget = state.budget();
    let details: Vec<(Value, Result<Repo, String>)> = stream::iter(candidates)
        .map(|m| {
            let client = client.clone();
            async move {
                let id = m["id"].as_str().unwrap_or_default().to_string();
                let r = repo(&client, &id).await;
                (m, r)
            }
        })
        .buffered(6)
        .collect()
        .await;
    let mut out = HubResults { models: Vec::new(), hidden: 0 };
    for (m, r) in details {
        let Ok(r) = r else { continue };
        let model = describe(&m, &r, &budget, all);
        if model.files.is_empty() {
            out.hidden += 1;
        } else {
            out.models.push(model);
        }
    }
    Ok(out)
}

/// The spec to install one file of a repository with (kept in the list so
/// the Models page shows its progress). Its picture reader comes along.
pub async fn prepare(state: &Arc<AppState>, repo_id: &str, file: &str) -> Result<ModelSpec, String> {
    let client = client(state)?;
    let r = repo(&client, repo_id).await?;
    if r.info["gated"] != false && !r.info["gated"].is_null() {
        return Err("That model needs a Hugging Face account to download, which SulcusAI doesn't use.".into());
    }
    let f = model_files(&r).find(|f| f.path == file).ok_or("That file isn't in the model's repository anymore.")?;
    let url = |path: &str| {
        let mut u = reqwest::Url::parse(SITE).map_err(|e| e.to_string())?;
        u.path_segments_mut().map_err(|_| "bad address".to_string())?.pop_if_empty().extend(repo_id.split('/')).push("resolve").push("main").extend(path.split('/'));
        Ok::<String, String>(u.to_string())
    };
    let quant = quant_of(&f.path);
    let variant = Variant { quality: quality_of(&quant), quant, file: base_name(&f.path).to_string(), url: url(&f.path)?, size: f.size, sha256: f.sha256.clone() };
    // The picture reader: the full-precision one if there's a choice, else the smallest.
    let mut projectors: Vec<&RemoteFile> = r.files.iter().filter(|p| is_projector(&p.path) && p.path.to_lowercase().ends_with(".gguf")).collect();
    projectors.sort_by_key(|p| (!p.path.to_lowercase().contains("f16"), p.size));
    let vision = match projectors.first() {
        Some(p) => Some(VisionFile { file: base_name(&p.path).to_string(), url: url(&p.path)?, size: p.size, sha256: p.sha256.clone() }),
        None => None,
    };
    let spec = spec_for(repo_id, &r.info, vec![variant], vision, model_id(repo_id, file));
    remember(&state.db.lock().unwrap(), &spec)?;
    Ok(spec)
}

/// Once downloaded, the file's header replaces the estimates (layers,
/// context, tools, and the real quantization).
pub fn refine(state: &AppState, spec: &mut ModelSpec, file: &Path) {
    let Ok(real) = local::read_spec(file) else { return };
    spec.arch = real.arch;
    spec.default_ctx = real.default_ctx;
    spec.tools = real.tools;
    spec.params_b = real.params_b;
    if let (Some(v), Some(r)) = (spec.variants.first_mut(), real.variants.first()) {
        v.quant = r.quant.clone();
        v.quality = r.quality.clone();
    }
    remember(&state.db.lock().unwrap(), spec).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_tell_the_quantization() {
        assert_eq!(quant_of("gpt-oss-20b-Q4_K_M.gguf"), "Q4_K_M");
        assert_eq!(quant_of("Qwen3-8B-UD-Q4_K_XL.gguf"), "Q4_K_XL");
        assert_eq!(quant_of("Q8_0/model-Q8_0.gguf"), "Q8_0");
        assert_eq!(quant_of("gemma-3-4b-it-IQ4_XS.gguf"), "IQ4_XS");
        assert_eq!(quant_of("gpt-oss-20b-mxfp4.gguf"), "MXFP4");
        assert_eq!(quant_of("phi-4-fp16.gguf"), "F16");
        assert_eq!(quant_of("model.gguf"), "GGUF");
        assert_eq!(quality_of("Q4_K_XL"), "Good");
        assert_eq!(quality_of("Q8_0"), "Best");
    }

    #[test]
    fn split_files_and_picture_readers_are_told_apart() {
        assert!(is_split("Q4_K_M/big-Q4_K_M-00001-of-00003.gguf"));
        assert!(!is_split("model-Q4_K_M.gguf"));
        assert!(is_projector("mmproj-F16.gguf") && is_projector("sub/Model-mmproj-f16.gguf"));
        assert_eq!(display_name("unsloth/gpt-oss-20b-GGUF"), "gpt-oss-20b");
        assert_eq!(display_name("bartowski/Qwen_Qwen3-8B-GGUF"), "Qwen_Qwen3-8B");
        assert!(is_helper("eagle3-gpt-oss-20b-BF16.gguf") && is_helper("GGUF/mtp-Model-Q8_0.gguf") && is_helper("Model-draft-Q8_0.gguf") && !is_helper("gpt-oss-20b-MXFP4.gguf"));
    }

    #[test]
    fn mixture_of_experts_are_estimated_by_what_runs_per_token() {
        assert_eq!(active_b("unsloth/Qwen3-30B-A3B-GGUF"), Some(3.0));
        assert_eq!(active_b("ggml-org/gpt-oss-20b-GGUF"), None);
        let moe = estimate_arch(30.0, "qwen3moe", "Qwen3-30B-A3B", 40960);
        assert!((moe.active_fraction - 0.19).abs() < 0.01);
        assert!(estimate_arch(20.9, "gpt-oss", "x", 131072).active_fraction < 0.3);
        assert_eq!(estimate_arch(8.0, "llama", "x", 8192).active_fraction, 1.0);
    }

    /// A made-up search result: only the files the budget can run are kept,
    /// split files and the picture reader aren't offered as models.
    #[test]
    fn only_files_this_pc_can_run_are_shown() {
        const GIB: u64 = 1 << 30;
        let info = serde_json::json!({ "gguf": { "total": 8.0e9, "architecture": "llama", "context_length": 32768, "chat_template": "{% if tools %}" } });
        let f = |path: &str, size: u64| RemoteFile { path: path.into(), size, sha256: "x".into() };
        let repo = Repo { info: info.clone(), files: vec![f("m-Q4_K_M.gguf", 5 * GIB), f("m-Q8_0.gguf", 9 * GIB), f("m-F16.gguf", 16 * GIB), f("m-Q4_K_M-00001-of-00002.gguf", 3 * GIB), f("mmproj-F16.gguf", GIB), f("eagle3-m-BF16.gguf", GIB), f("m-tiny.gguf", GIB / 2)] };
        let m = serde_json::json!({ "id": "someone/m-GGUF", "downloads": 10, "likes": 2, "gguf": info["gguf"] });
        let budget = Budget { vram: 8 * GIB, ram: 4 * GIB, disk: 500 * GIB, gpu_bw_gbs: 400.0, cpu_bw_gbs: 60.0 };
        let shown = describe(&m, &repo, &budget, false);
        let files: Vec<&str> = shown.files.iter().map(|f| f.file.as_str()).collect();
        assert_eq!(files, ["m-Q4_K_M.gguf", "m-Q8_0.gguf"], "F16 is too big; split parts, picture readers, helpers and too-small files aren't models");
        assert_eq!(shown.files[0].placement, Some(Placement::Gpu));
        assert!(shown.vision);
        assert_eq!(shown.recommended.as_deref(), Some("m-Q4_K_M.gguf"));
        let tiny = Budget { vram: 0, ram: 2 * GIB, disk: 500 * GIB, gpu_bw_gbs: 0.0, cpu_bw_gbs: 30.0 };
        assert!(describe(&m, &repo, &tiny, false).files.is_empty(), "nothing fits: the model is hidden");
        // Browsing everything: the files it can't run come too, without a placement.
        let every = describe(&m, &repo, &tiny, true);
        assert_eq!(every.files.len(), 3, "Q4_K_M, Q8_0 and F16; still no split part, picture reader or helper");
        assert!(every.files.iter().all(|f| f.placement.is_none() && f.needed > f.size));
        assert_eq!(every.recommended, None);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Against huggingface.co with this PC's hardware: a search shows only
    /// runnable files, and a small model downloads, checks out and is read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn search_and_fetch_from_hugging_face() {
        let state = crate::e2e::scratch_state();
        println!("budget: {:?}", state.budget());
        for (q, sort) in [("gpt-oss", "downloads"), ("", "trending"), ("qwen3", "downloads")] {
            let r = search(&state, q, sort, false).await.unwrap();
            println!("\n“{q}” ({sort}): {} shown, {} hidden", r.models.len(), r.hidden);
            for m in r.models.iter().take(4) {
                let files: Vec<String> = m.files.iter().map(|f| format!("{} {:.1}GB {:?}", f.quant, f.size as f64 / 1e9, f.placement)).collect();
                println!("  {} ({:?}B, rec {:?}): {}", m.id, m.params_b, m.recommended, files.join(", "));
            }
            assert!(r.models.iter().all(|m| !m.files.is_empty()));
        }
        let spec = prepare(&state, "unsloth/Qwen3-0.6B-GGUF", "Qwen3-0.6B-Q4_K_M.gguf").await.unwrap();
        let v = &spec.variants[0];
        println!("\nprepared {} {} {} {}", spec.id, v.quant, v.url, v.size);
        let dest = state.paths.models.join(&spec.id).join(&v.file);
        let client = client(&state).unwrap();
        crate::download::fetch_verified(&client, &v.url, &dest, v.size, &v.sha256, &std::sync::atomic::AtomicBool::new(false), |_, _| {}).await.unwrap();
        let mut spec = spec;
        refine(&state, &mut spec, &dest);
        println!("refined: {:?} ctx {} tools {}", spec.arch, spec.default_ctx, spec.tools);
        assert_eq!(spec.arch.n_layer, 28, "Qwen3 0.6B has 28 layers");
        assert!(spec.tools);
        assert!(list(&state.db.lock().unwrap()).iter().any(|m| m.id == spec.id));
    }
}

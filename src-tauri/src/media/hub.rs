// SPDX-License-Identifier: AGPL-3.0-only
//! Picture, video and music models from Hugging Face. Each family a catalog
//! model opens (`hub` in the catalog: SDXL, Z-Image, FLUX.2 klein, Wan,
//! ACE-Step…) is searched by Hugging Face's "base model" link, so only
//! models built on one SulcusAI can run are shown. Installing one takes the
//! catalog model's files and swaps in the downloaded one for its role (the
//! whole checkpoint, or just the diffusion part, sharing the text reader and
//! decoder the catalog model uses). Models that need an account (gated),
//! add-ons (LoRAs, ControlNets) and files split in parts are left out.

use std::collections::HashSet;
use std::sync::Arc;

use futures_util::{stream, StreamExt};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{fit, missing_bytes, Kind, MediaFile, MediaModel};
use crate::catalog::{Budget, License, GIB};
use crate::hub::{self as hf, RemoteFile, Repo};
use crate::{db, AppState};

const KEY: &str = "hub_media_models";
/// Repositories looked at per family (each needs one more request for its files).
const PER_FAMILY: usize = 16;

/// What a catalog model opens to Hugging Face: models built on it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct HubFamily {
    /// Shown on results: "SDXL", "Z-Image".
    pub name: String,
    /// Hugging Face's ids of the base model (their "base_model" link).
    pub bases: Vec<String>,
    /// Tags that mark a model of this family when it names no base model
    /// ("diffusers:StableDiffusionXLPipeline").
    pub tags: Vec<String>,
    /// The file a model from Hugging Face replaces: "model" (the whole
    /// checkpoint), "diffusion" or "dit" (music).
    pub role: String,
    /// File sizes that can be the model (smaller ones are add-ons or parts).
    pub min_size: u64,
    pub max_size: u64,
    /// Fine-tunes differ in steps: "lightning"/"turbo" ones take a few, the
    /// rest more (checkpoints of SDXL and the like).
    pub tune_steps: bool,
    /// A file's name must have all of these (lowercase).
    pub require: Vec<String>,
    /// …and one of these, when any are given ("1.3b", "1_3b").
    pub any_of: Vec<String>,
    /// Names with any of these are other models in the same repository
    /// ("14b" beside a 1.3B one) or versions that need other settings.
    pub avoid: Vec<String>,
}

/// Models installed (or being installed) from Hugging Face.
pub fn list(conn: &Connection) -> Vec<MediaModel> {
    db::get(conn, KEY).unwrap_or_default()
}

fn remember(conn: &Connection, m: &MediaModel) -> Result<(), String> {
    let mut all = list(conn);
    all.retain(|x| x.id != m.id);
    all.push(m.clone());
    db::set(conn, KEY, &all)
}

pub fn forget(conn: &Connection, id: &str) -> Result<(), String> {
    let mut all = list(conn);
    all.retain(|x| x.id != id);
    db::set(conn, KEY, &all)
}

#[derive(Debug, Serialize)]
pub struct HubMediaFile {
    pub file: String,
    pub model_id: String,
    /// "Q4_K", "FP16".
    pub quant: String,
    pub size: u64,
    /// What's left to download (the shared files may be here already).
    pub download: u64,
    pub runnable: bool,
    pub on_gpu: bool,
    pub est_secs: f64,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HubMediaModel {
    pub id: String,
    pub name: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    pub updated: Option<String>,
    pub url: String,
    /// The family it belongs to ("SDXL") and the catalog model it builds on.
    pub family: String,
    pub base_model: String,
    pub license: Option<String>,
    pub files: Vec<HubMediaFile>,
    pub recommended: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HubMediaResults {
    pub models: Vec<HubMediaModel>,
    /// Results left out: nothing of theirs runs here, or only in formats SulcusAI can't use.
    pub hidden: usize,
}

/// Checksums of the catalog's own files.
fn known_files(state: &AppState) -> HashSet<String> {
    state.catalog.media.models.iter().flat_map(|m| m.files.iter().map(|f| f.sha256.clone())).collect()
}

/// The catalog models that open a family, for one kind.
fn families(state: &AppState, kind: Kind) -> Vec<MediaModel> {
    state.catalog.media.models.iter().filter(|m| m.kind == kind && m.hub.is_some()).cloned().collect()
}

/// The families a repository belongs to, from its base-model links (one
/// base can open several: ACE-Step's standard and XL music models).
fn families_of<'a>(info: &Value, donors: &'a [MediaModel]) -> Vec<&'a MediaModel> {
    let tags: Vec<&str> = info["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).collect();
    let id = info["id"].as_str().unwrap_or_default();
    donors.iter().filter(|d| {
        d.hub.as_ref().is_some_and(|h| {
            h.tags.iter().any(|tag| tags.iter().any(|t| t.eq_ignore_ascii_case(tag))) || h.bases.iter().any(|b| {
                id.eq_ignore_ascii_case(b)
                    || tags.iter().any(|t| {
                        // "base_model:X", "base_model:finetune:X", "base_model:quantized:X", "base_model:merge:X";
                        // not "base_model:adapter:X" (LoRAs and ControlNets need the base too).
                        let Some(rest) = t.strip_prefix("base_model:") else { return false };
                        let (how, of) = rest.split_once(':').filter(|(h, _)| !h.contains('/')).unwrap_or(("", rest));
                        how != "adapter" && of.eq_ignore_ascii_case(b)
                    })
            })
        })
    })
    .collect()
}

/// Each file a repository offers, with the catalog model it stands in for
/// (not files a catalog model already uses: those are in its list).
fn offered<'a, 'b>(repo: &'a Repo, donors: &[&'b MediaModel], known: &HashSet<String>) -> Vec<(&'a RemoteFile, &'b MediaModel)> {
    let mut out: Vec<(&RemoteFile, &MediaModel)> = Vec::new();
    for d in donors {
        let Some(h) = &d.hub else { continue };
        for f in candidates(repo, h) {
            if !known.contains(&f.sha256) && !out.iter().any(|(x, _)| x.path == f.path) {
                out.push((f, d));
            }
        }
    }
    out.sort_by_key(|(f, _)| f.size);
    out
}

/// Add-ons and parts that aren't a model of their own.
fn is_part(name: &str) -> bool {
    let n = name.to_lowercase();
    ["lora", "vae", "controlnet", "control_", "refiner", "inpaint", "encoder", "clip", "t5", "umt5", "text_", "mmproj", "llm", "embedding", "ip-adapter", "ipadapter", "motion", "adapter"]
        .iter()
        .any(|w| n.contains(w))
}

/// "…-00001-of-00003.safetensors": one part of a model split across files.
fn is_split(path: &str) -> bool {
    let name = hf::base_name(path).to_lowercase();
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    let mut it = stem.rsplit('-');
    let digits = |s: &str| !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit());
    matches!((it.next(), it.next(), it.next()), (Some(t), Some("of"), Some(p)) if digits(t) && digits(p))
}

/// The files of a repository that can stand in for the family's role.
fn candidates<'a>(repo: &'a Repo, h: &HubFamily) -> Vec<&'a RemoteFile> {
    let mut out: Vec<&RemoteFile> = repo
        .files
        .iter()
        .filter(|f| {
            let lower = f.path.to_lowercase();
            let name = hf::base_name(&f.path);
            let ext_ok = match h.role.as_str() {
                // A whole checkpoint: one file at the top of the repository.
                "model" => lower.ends_with(".safetensors") && !f.path.contains('/'),
                // Music: acestep.cpp's own GGUF files.
                "dit" => lower.ends_with(".gguf") && lower.contains("acestep") && !lower.contains("-lm-") && !lower.contains("5hz"),
                // A diffusion model: at the top, or in a folder named for it
                // (not a folder holding some other model).
                _ => {
                    let folder_ok = !f.path.contains('/') || lower.contains("diffusion") || lower.contains("transformer") || lower.contains("lownoise") || lower.contains("gguf");
                    (lower.ends_with(".gguf") || lower.ends_with(".safetensors")) && folder_ok && !name.to_lowercase().starts_with("diffusion_pytorch_model")
                }
            };
            let words_ok = h.require.iter().all(|w| lower.contains(w.as_str()))
                && (h.any_of.is_empty() || h.any_of.iter().any(|w| lower.contains(w.as_str())))
                && !h.avoid.iter().any(|w| lower.contains(w.as_str()));
            ext_ok && words_ok && !is_split(&f.path) && !is_part(name) && f.size >= h.min_size && (h.max_size == 0 || f.size <= h.max_size) && !f.sha256.is_empty()
        })
        .collect();
    out.sort_by_key(|f| f.size);
    out
}

/// "Q4_K", "Q8_0", "FP16", "BF16", "FP8" from a file name.
fn quant_label(path: &str) -> String {
    if path.to_lowercase().ends_with(".gguf") {
        return hf::quant_of(path);
    }
    let up = hf::base_name(path).to_uppercase();
    for (k, v) in [("FP8", "FP8"), ("BF16", "BF16"), ("FP16", "FP16"), ("F16", "FP16"), ("FP32", "FP32"), ("F32", "FP32")] {
        if up.contains(k) {
            return v.into();
        }
    }
    "Full".into()
}

/// The id a file installs as: the same for the same file, every time.
fn model_id(repo_id: &str, file: &str) -> String {
    format!("hfm-{}", &hex::encode(Sha256::digest(format!("{repo_id}/{file}").as_bytes()))[..12])
}

/// Fine-tunes made to take only a few steps.
fn is_fast(name: &str) -> bool {
    let n = name.to_lowercase();
    ["lightning", "turbo", "hyper", "lcm", "dmd", "4step", "8step", "4-step", "8-step", "flash"].iter().any(|w| n.contains(w))
}

/// The catalog model, with the downloaded file in place of its role's.
fn derive(donor: &MediaModel, repo_id: &str, info: &Value, f: &RemoteFile, url: String) -> MediaModel {
    let h = donor.hub.clone().unwrap_or_default();
    let mut m = donor.clone();
    let id = model_id(repo_id, &f.path);
    let old = donor.file(&h.role).map_or(f.size, |x| x.size);
    let quant = quant_label(&f.path);
    m.id = id.clone();
    m.name = format!("{} ({quant})", hf::display_name(repo_id));
    m.publisher = info["author"].as_str().map(str::to_string).unwrap_or_else(|| repo_id.split('/').next().unwrap_or("").to_string());
    m.source = format!("{}/{repo_id}", hf::SITE);
    m.description = format!("From Hugging Face ({repo_id}), built on {}. It's downloaded to this PC; removing it deletes its own file.", h.name);
    let license = info["cardData"]["license"].as_str().map(str::to_string);
    m.license = License {
        name: license.unwrap_or_else(|| "See the model's page".into()),
        url: m.source.clone(),
        commercial: false,
        note: Some("Check the license on the model's page before using it for work.".into()),
    };
    // Its own file name, so two repositories' "model.safetensors" never collide.
    let local = format!("{id}-{}", hf::base_name(&f.path));
    for file in m.files.iter_mut().filter(|x| x.role == h.role) {
        *file = MediaFile { role: h.role.clone(), file: local.clone(), url: url.clone(), size: f.size, sha256: f.sha256.clone() };
    }
    // Memory and time grow with the file, from what the catalog model needs.
    let extra_gib = (f.size as f64 - old as f64) / GIB as f64;
    m.min_vram_gb = (donor.min_vram_gb + extra_gib * 0.8).max(2.0).round();
    m.secs = (donor.secs * (f.size as f64 / old.max(1) as f64).clamp(0.5, 3.0).sqrt()).round();
    if h.tune_steps {
        let fast = is_fast(repo_id) || is_fast(&f.path);
        let donor_fast = donor.defaults.steps <= 10;
        if fast != donor_fast {
            if fast {
                m.defaults.steps = 6;
                m.defaults.cfg = 1.5;
                m.defaults.sampler = "dpm++2m".into();
                m.secs = (m.secs * 0.3).round();
            } else {
                m.defaults.steps = 28;
                m.defaults.cfg = 5.5;
                m.defaults.sampler = "dpm++2m".into();
                m.secs = (m.secs * 3.5).round();
            }
        }
    }
    m.hub = None;
    m.from_hub = Some(repo_id.to_string());
    m
}

fn file_url(repo_id: &str, path: &str) -> Result<String, String> {
    let mut u = reqwest::Url::parse(hf::SITE).map_err(|e| e.to_string())?;
    u.path_segments_mut().map_err(|_| "bad address".to_string())?.pop_if_empty().extend(repo_id.split('/')).push("resolve").push("main").extend(path.split('/'));
    Ok(u.to_string())
}

fn open(info: &Value) -> bool {
    info["gated"].is_null() || info["gated"] == false
}

/// Searches Hugging Face for models of one kind ("image", "video", "music")
/// built on a family SulcusAI runs. `all` keeps files this PC can't run.
pub async fn search(state: &Arc<AppState>, kind: &str, query: &str, sort: &str, all: bool) -> Result<HubMediaResults, String> {
    let kind = match kind {
        "video" => Kind::Video,
        "music" => Kind::Music,
        _ => Kind::Image,
    };
    let donors = families(state, kind);
    if donors.is_empty() {
        return Ok(HubMediaResults { models: Vec::new(), hidden: 0 });
    }
    let client = hf::client(state)?;
    let sort_key = match sort {
        "trending" => "trendingScore",
        "newest" => "lastModified",
        _ => "downloads",
    };
    // One search per base model, together.
    let bases: Vec<String> = donors.iter().flat_map(|d| d.hub.as_ref().map(|h| h.bases.clone()).unwrap_or_default()).collect();
    state.log("network", "Searched Hugging Face for picture, video or music models (huggingface.co)");
    let pages: Vec<Result<Vec<Value>, String>> = stream::iter(bases)
        .map(|base| {
            let client = client.clone();
            let query = query.trim().to_string();
            async move {
                let mut url = reqwest::Url::parse(hf::API).map_err(|e| e.to_string())?;
                {
                    let mut q = url.query_pairs_mut();
                    if !query.is_empty() {
                        q.append_pair("search", &query);
                    }
                    q.append_pair("filter", &format!("base_model:{base}"));
                    q.append_pair("sort", sort_key);
                    q.append_pair("direction", "-1");
                    q.append_pair("limit", &PER_FAMILY.to_string());
                    for e in ["downloads", "likes", "lastModified", "gated", "tags", "trendingScore"] {
                        q.append_pair("expand[]", e);
                    }
                }
                client
                    .get(url)
                    .send()
                    .await
                    .map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?
                    .error_for_status()
                    .map_err(|e| format!("Hugging Face answered: {e}"))?
                    .json::<Vec<Value>>()
                    .await
                    .map_err(|e| e.to_string())
            }
        })
        .buffer_unordered(4)
        .collect()
        .await;
    let mut seen = HashSet::new();
    let mut found: Vec<Value> = Vec::new();
    let mut first_error = None;
    for p in pages {
        match p {
            Ok(list) => found.extend(list.into_iter().filter(|m| open(m) && m["id"].as_str().is_some_and(|id| seen.insert(id.to_string())))),
            Err(e) => first_error = first_error.or(Some(e)),
        }
    }
    if found.is_empty() {
        if let Some(e) = first_error {
            return Err(e);
        }
    }
    let by = |m: &Value| match sort_key {
        "lastModified" => 0,
        "trendingScore" => m["trendingScore"].as_i64().unwrap_or(0),
        _ => m["downloads"].as_i64().unwrap_or(0),
    };
    if sort_key == "lastModified" {
        found.sort_by(|a, b| b["lastModified"].as_str().unwrap_or("").cmp(a["lastModified"].as_str().unwrap_or("")));
    } else {
        found.sort_by_key(|m| std::cmp::Reverse(by(m)));
    }
    found.truncate(30);
    let details: Vec<(Value, Result<Repo, String>)> = stream::iter(found)
        .map(|m| {
            let client = client.clone();
            async move {
                let id = m["id"].as_str().unwrap_or_default().to_string();
                let r = hf::repo(&client, &id).await;
                (m, r)
            }
        })
        .buffered(6)
        .collect()
        .await;
    let known = known_files(state);
    let mut out = HubMediaResults { models: Vec::new(), hidden: 0 };
    for (m, r) in details {
        let Ok(r) = r else { continue };
        match describe(state, &m, &r, &donors, &known, all) {
            Some((model, _)) => out.models.push(model),
            None => out.hidden += 1,
        }
    }
    Ok(out)
}

/// What the Studio shows for one repository: the files this PC can run (or,
/// with `all`, every file), with the kind of model it is. None when it isn't
/// built on a family SulcusAI runs, or no file of it can be used.
fn describe(state: &AppState, m: &Value, r: &Repo, donors: &[MediaModel], known: &HashSet<String>, all: bool) -> Option<(HubMediaModel, Kind)> {
    let hw = state.hardware.read().unwrap().clone();
    let budget = Budget::from_hardware(&hw);
    // Search results lack the license; the repository's details have it.
    let mut info = r.info.clone();
    if info["tags"].is_null() {
        info["tags"] = m["tags"].clone();
    }
    let mine = families_of(&info, donors);
    let first = mine.first()?;
    let h = first.hub.clone().unwrap_or_default();
    let repo_id = m["id"].as_str().or(info["id"].as_str()).unwrap_or_default().to_string();
    let files: Vec<HubMediaFile> = offered(r, &mine, known)
        .into_iter()
        .map(|(f, donor)| {
            let spec = derive(donor, &repo_id, &info, f, String::new());
            let missing = missing_bytes(&state.paths, &spec);
            let fit = fit(&spec, &hw, &budget, missing, None);
            HubMediaFile {
                file: f.path.clone(),
                model_id: spec.id.clone(),
                quant: quant_label(&f.path),
                size: f.size,
                download: missing,
                runnable: fit.runnable && fit.disk_ok,
                on_gpu: fit.on_gpu,
                est_secs: fit.est_secs,
                reason: fit.reason.clone().or_else(|| (!fit.disk_ok).then(|| "needs more free disk space".into())),
            }
        })
        .filter(|f| all || f.runnable)
        .collect();
    if files.is_empty() {
        return None;
    }
    let recommended = recommend(&files);
    let model = HubMediaModel {
        name: hf::display_name(&repo_id),
        author: repo_id.split('/').next().unwrap_or("").to_string(),
        downloads: m["downloads"].as_u64().unwrap_or(0),
        likes: m["likes"].as_u64().unwrap_or(0),
        updated: m["lastModified"].as_str().map(|d| d.chars().take(10).collect()),
        url: format!("{}/{repo_id}", hf::SITE),
        family: h.name.clone(),
        base_model: first.name.clone(),
        license: info["cardData"]["license"].as_str().map(str::to_string),
        files,
        recommended,
        id: repo_id,
    };
    Some((model, first.kind))
}

/// A repository someone linked to, as a picture, video or music model, with
/// every file (the ones this PC can't run say why): None when it isn't one.
pub fn from_link(state: &AppState, r: &Repo) -> Option<(HubMediaModel, &'static str)> {
    let donors: Vec<MediaModel> = [Kind::Image, Kind::Video, Kind::Music].into_iter().flat_map(|k| families(state, k)).collect();
    let (model, kind) = describe(state, &r.info, r, &donors, &known_files(state), true)?;
    let kind = match kind {
        Kind::Video => "video",
        Kind::Music => "music",
        _ => "image",
    };
    Some((model, kind))
}

/// The file that suits this PC best: the biggest one that runs on the card,
/// preferring compressed ones (Q8 and Q6 look the same as full size at half
/// the download), else the biggest that runs at all.
fn recommend(files: &[HubMediaFile]) -> Option<String> {
    let compressed = |f: &&HubMediaFile| f.quant.starts_with('Q') || f.quant.starts_with("IQ");
    let runs: Vec<&HubMediaFile> = files.iter().filter(|f| f.runnable).collect();
    let on_card: Vec<&HubMediaFile> = runs.iter().copied().filter(|f| f.on_gpu).collect();
    let pool = if on_card.is_empty() { runs } else { on_card };
    pool.iter().copied().filter(compressed).max_by_key(|f| f.size).or_else(|| pool.iter().copied().max_by_key(|f| f.size)).map(|f| f.file.clone())
}

/// The model to install one file of a repository as (kept in the list so
/// the Studio shows its progress and can use it).
pub async fn prepare(state: &Arc<AppState>, repo_id: &str, file: &str) -> Result<MediaModel, String> {
    let client = hf::client(state)?;
    let r = hf::repo(&client, repo_id).await?;
    if !open(&r.info) {
        return Err("That model needs a Hugging Face account to download, which SulcusAI doesn't use.".into());
    }
    let donors: Vec<MediaModel> = [Kind::Image, Kind::Video, Kind::Music].into_iter().flat_map(|k| families(state, k)).collect();
    let mine = families_of(&r.info, &donors);
    if mine.is_empty() {
        return Err("SulcusAI can't tell which kind of model that is, so it can't run it.".into());
    }
    let (f, donor) = offered(&r, &mine, &known_files(state)).into_iter().find(|(f, _)| f.path == file).ok_or("That file isn't in the model's repository anymore.")?;
    let spec = derive(donor, repo_id, &r.info, f, file_url(repo_id, &f.path)?);
    remember(&state.db.lock().unwrap(), &spec)?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn donor() -> MediaModel {
        serde_json::from_value(json!({
            "id": "realvisxl-v5-lightning", "name": "RealVisXL V5.0 Lightning", "publisher": "SG161222", "source": "", "description": "",
            "license": { "name": "openrail++", "url": "", "commercial": true },
            "kind": "image", "can": ["generate", "fill", "extend"],
            "files": [
                { "role": "model", "file": "RealVisXL_V5.0_Lightning_fp16.safetensors", "url": "u", "size": 6_938_065_512u64, "sha256": "a" },
                { "role": "vae", "file": "sdxl_vae_fp16_fix.safetensors", "url": "u", "size": 334_641_162u64, "sha256": "b" }
            ],
            "min_vram_gb": 6.0, "cpu": false, "secs": 12.0, "quality": 9,
            "defaults": { "steps": 6, "cfg": 1.5, "sampler": "dpm++2m", "width": 1024, "height": 1024 },
            "hub": { "name": "SDXL", "bases": ["stabilityai/stable-diffusion-xl-base-1.0"], "role": "model", "min_size": 2_000_000_000u64, "max_size": 8_000_000_000u64, "tune_steps": true }
        }))
        .unwrap()
    }

    #[test]
    fn families_follow_base_model_links_but_not_add_ons() {
        let d = vec![donor()];
        let fine = json!({ "id": "RunDiffusion/Juggernaut-XL-v9", "tags": ["base_model:stabilityai/stable-diffusion-xl-base-1.0", "base_model:finetune:stabilityai/stable-diffusion-xl-base-1.0"] });
        let lora = json!({ "id": "latent-consistency/lcm-lora-sdxl", "tags": ["lora", "base_model:adapter:stabilityai/stable-diffusion-xl-base-1.0"] });
        let other = json!({ "id": "x/y", "tags": ["base_model:black-forest-labs/FLUX.1-dev"] });
        assert_eq!(families_of(&fine, &d).len(), 1);
        assert!(families_of(&lora, &d).is_empty());
        assert!(families_of(&other, &d).is_empty());
    }

    #[test]
    fn a_checkpoint_replaces_the_model_and_keeps_the_shared_decoder() {
        let d = donor();
        let repo = Repo {
            info: json!({ "id": "RunDiffusion/Juggernaut-XL-v9", "author": "RunDiffusion", "cardData": { "license": "creativeml-openrail-m" } }),
            files: vec![
                RemoteFile { path: "Juggernaut-XL_v9_RunDiffusionPhoto_v2.safetensors".into(), size: 7_105_000_000, sha256: "c".into() },
                RemoteFile { path: "unet/diffusion_pytorch_model.fp16.safetensors".into(), size: 5_140_000_000, sha256: "d".into() },
                RemoteFile { path: "juggernaut_lora.safetensors".into(), size: 2_300_000_000, sha256: "e".into() },
                RemoteFile { path: "Juggernaut_fp32.safetensors".into(), size: 13_900_000_000, sha256: "f".into() },
                RemoteFile { path: "model-00001-of-00002.safetensors".into(), size: 4_000_000_000, sha256: "g".into() },
            ],
        };
        let h = d.hub.clone().unwrap();
        let c = candidates(&repo, &h);
        assert_eq!(c.len(), 1, "not a part in a folder, a LoRA or a file too big");
        let m = derive(&d, "RunDiffusion/Juggernaut-XL-v9", &repo.info, c[0], "url".into());
        assert!(m.id.starts_with("hfm-") && m.from_hub.as_deref() == Some("RunDiffusion/Juggernaut-XL-v9"));
        assert!(m.file("model").unwrap().file.ends_with("Juggernaut-XL_v9_RunDiffusionPhoto_v2.safetensors"));
        assert_eq!(m.file("vae").unwrap().file, "sdxl_vae_fp16_fix.safetensors");
        assert_eq!(m.defaults.steps, 28, "a regular fine-tune of a fast model takes the usual steps");
        assert!(m.hub.is_none());
        assert_eq!(m.license.name, "creativeml-openrail-m");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn pasted_links_find_the_right_kind_of_model() {
        let state = crate::e2e::scratch_state();
        let client = hf::client(&state).unwrap();
        for (link, kind) in [
            ("https://huggingface.co/SG161222/RealVisXL_V5.0/blob/main/RealVisXL_V5.0_fp16.safetensors", Some("image")),
            ("https://huggingface.co/QuantStack/Wan2.2-TI2V-5B-GGUF", Some("video")),
            ("hf.co/unsloth/Qwen3-0.6B-GGUF", None),
        ] {
            let (repo, file) = hf::parse_link(link).unwrap();
            let r = hf::repo(&client, &repo).await.unwrap();
            let media = from_link(&state, &r);
            println!("{link} -> media {:?} ({} files), chat {}", media.as_ref().map(|m| m.1), media.as_ref().map_or(0, |m| m.0.files.len()), hf::is_chat(&r));
            assert_eq!(media.as_ref().map(|m| m.1), kind);
            if kind.is_none() {
                assert!(hf::is_chat(&r), "a chat model");
            }
            if let (Some(f), Some((m, _))) = (file, &media) {
                assert!(m.files.iter().any(|x| x.file == f), "the linked file is offered");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn search_hugging_face_for_media_models() {
        let state = crate::e2e::scratch_state();
        for (kind, q, sort) in [("image", "", "downloads"), ("image", "juggernaut", "downloads"), ("video", "", "downloads"), ("music", "", "downloads"), ("image", "", "trending")] {
            let r = search(&state, kind, q, sort, false).await.unwrap();
            println!("
{kind} “{q}” ({sort}): {} shown, {} hidden", r.models.len(), r.hidden);
            for m in r.models.iter().take(8) {
                let files: Vec<String> = m.files.iter().map(|f| format!("{} {:.1}GB{}", f.quant, f.size as f64 / 1e9, if f.on_gpu { " card" } else { "" })).collect();
                println!("  [{}] {} rec {:?}: {}", m.family, m.id, m.recommended, files.join(", "));
            }
            assert!(r.models.iter().all(|m| !m.files.is_empty()));
        }
        let spec = prepare(&state, "RunDiffusion/Juggernaut-XL-v9", "Juggernaut-XL_v9_RunDiffusionPhoto_v2.safetensors").await.unwrap();
        println!("
prepared {} {:?} {:?}", spec.id, spec.files.iter().map(|f| (&f.role, &f.file, f.size)).collect::<Vec<_>>(), spec.defaults);
        assert!(super::super::models(&state).iter().any(|m| m.id == spec.id));
    }
}

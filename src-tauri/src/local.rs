// SPDX-License-Identifier: AGPL-3.0-only
//! Chat models other apps downloaded (Ollama, LM Studio, Hugging Face, Jan,
//! GPT4All, or a .gguf in Downloads), used where they are: nothing is copied
//! and removing one here never deletes the other app's file. What a model is
//! comes from its GGUF header, so any model llama.cpp can run works, not
//! just the ones in the catalog.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::catalog::{self, Arch, License, LocalSource, ModelSpec, Ratings, Variant, VisionFile};
use crate::db::{self, InstalledModel};
use crate::found::{self, Entry};
use crate::gguf::{self, Value};
use crate::AppState;

const KEY: &str = "local_models";
/// Files the user stopped using here, so a later look doesn't add them back.
const HIDDEN_KEY: &str = "local_models_hidden";
/// Smaller .gguf files are tokenizers, adapters and the like.
const MIN_SIZE: u64 = 50 * 1024 * 1024;

pub fn list(conn: &Connection) -> Vec<ModelSpec> {
    db::get(conn, KEY).unwrap_or_default()
}

fn save(conn: &Connection, list: &[ModelSpec]) -> Result<(), String> {
    db::set(conn, KEY, &list)
}

fn hidden(conn: &Connection) -> Vec<String> {
    db::get(conn, HIDDEN_KEY).unwrap_or_default()
}

fn key(path: &Path) -> String {
    path.display().to_string().to_lowercase()
}

/// Stops using a model from another app (its file stays where it is).
pub fn forget(conn: &Connection, id: &str) -> Result<Option<ModelSpec>, String> {
    let mut all = list(conn);
    let Some(i) = all.iter().position(|m| m.id == id) else { return Ok(None) };
    let spec = all.remove(i);
    save(conn, &all)?;
    if let Some(l) = &spec.local {
        let mut h = hidden(conn);
        h.push(key(Path::new(&l.path)));
        db::set(conn, HIDDEN_KEY, &h)?;
    }
    Ok(Some(spec))
}

struct Candidate {
    app: &'static str,
    path: PathBuf,
    /// Ollama's name for it, e.g. "qwen2.5-coder:14b".
    name: Option<String>,
    /// Its picture encoder, if it comes with one.
    projector: Option<PathBuf>,
}

/// Ollama keeps a manifest per model; its layers point at hash-named blobs.
fn ollama(models: &Path) -> Vec<Candidate> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth > 1 {
                    walk(&p, depth - 1, out);
                }
            } else if out.len() < 1000 {
                out.push(p);
            }
        }
    }
    let root = models.join("manifests");
    let mut files = Vec::new();
    walk(&root, 5, &mut files);
    let blob = |digest: &str| models.join("blobs").join(digest.replace(':', "-"));
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let layer = |kind: &str| {
            json["layers"].as_array()?.iter().find(|l| l["mediaType"].as_str() == Some(kind))?["digest"].as_str().map(blob)
        };
        let Some(model) = layer("application/vnd.ollama.image.model") else { continue };
        // registry/namespace/model/tag → "model:tag" for Ollama's own library.
        let parts: Vec<String> = f.strip_prefix(&root).map(|r| r.iter().map(|c| c.to_string_lossy().into_owned()).collect()).unwrap_or_default();
        let name = match parts.as_slice() {
            [reg, ns, m, tag] if reg == "registry.ollama.ai" && ns == "library" => format!("{m}:{tag}"),
            [reg, ns, m, tag] if reg == "registry.ollama.ai" => format!("{ns}/{m}:{tag}"),
            [.., m, tag] => format!("{m}:{tag}"),
            _ => continue,
        };
        out.push(Candidate { app: "Ollama", path: model, name: Some(name), projector: layer("application/vnd.ollama.image.projector") });
    }
    out
}

fn candidates(entries: &[Entry]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for s in found::sources() {
        if s.app == "Ollama" {
            // `sources` points at the blobs; the manifests are next to them.
            if let Some(models) = s.dir.parent() {
                out.extend(ollama(models));
            }
        }
    }
    for e in entries {
        if e.app == "Ollama" || !e.name.ends_with(".gguf") || e.name.starts_with("mmproj") || e.size < MIN_SIZE {
            continue;
        }
        // LM Studio and Hugging Face keep the picture encoder beside the model.
        let projector = e.path.parent().and_then(|dir| {
            std::fs::read_dir(dir).ok()?.flatten().map(|f| f.path()).find(|p| {
                let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                n.starts_with("mmproj") && n.ends_with(".gguf")
            })
        });
        out.push(Candidate { app: e.app, path: e.path.clone(), name: None, projector });
    }
    out
}

/// "14B", "30B-A3B", "1.5B", "8x7B", "500M" as billions of parameters.
fn size_label(label: &str) -> Option<f64> {
    let first = label.split(['-', '_']).next()?.to_uppercase();
    let (experts, rest) = match first.split_once('X') {
        Some((n, rest)) => (n.parse::<f64>().ok()?, rest.to_string()),
        None => (1.0, first),
    };
    let (num, scale) = if let Some(n) = rest.strip_suffix('B') {
        (n, 1.0)
    } else if let Some(n) = rest.strip_suffix('M') {
        (n, 0.001)
    } else if let Some(n) = rest.strip_suffix('T') {
        (n, 1000.0)
    } else {
        return None;
    };
    Some(experts * num.parse::<f64>().ok()? * scale)
}

fn describe(c: &Candidate) -> String {
    c.name.clone().unwrap_or_else(|| c.path.file_name().unwrap_or_default().to_string_lossy().into_owned())
}

/// What the model is, from its file. An error says why it can't be used.
fn spec_for(c: &Candidate) -> Result<ModelSpec, String> {
    let meta = gguf::read(&c.path).map_err(|_| "isn't a model file SulcusAI can read".to_string())?;
    let get = |k: &str| meta.get(k);
    let int = |k: &str| get(k).and_then(Value::int);
    let arch = get("general.architecture").and_then(Value::str).ok_or("isn't a model file SulcusAI can read")?.to_string();
    if arch == "clip" || get("general.type").and_then(Value::str) == Some("mmproj") {
        return Err("is a picture encoder, not a model".into());
    }
    let template = get("tokenizer.chat_template").and_then(Value::str).ok_or("has no chat format, so it isn't a chat model")?;
    // Ollama's newer picture models keep the picture parts in the same file,
    // in a layout only Ollama's own engine runs.
    if c.projector.is_none() && meta.keys().any(|k| k.starts_with(&format!("{arch}.vision.")) || k.starts_with("vision.")) {
        return Err("is built for Ollama's own engine, which SulcusAI can't run".into());
    }
    let a = |k: &str| int(&format!("{arch}.{k}"));
    let (Some(n_layer), Some(heads)) = (a("block_count"), a("attention.head_count")) else {
        return Err("isn't a kind of model SulcusAI understands".into());
    };
    let heads = heads.max(1);
    let kv_heads = a("attention.head_count_kv").unwrap_or(heads);
    let head_dim = a("attention.key_length").or_else(|| a("embedding_length").map(|e| e / heads)).unwrap_or(128);
    let max_ctx = a("context_length").unwrap_or(4096).clamp(2048, 131_072) as u32;
    let active_fraction = match (a("expert_count"), a("expert_used_count")) {
        // The experts hold most of the weights; only a few run per token.
        (Some(n), Some(used)) if n > 0 => 0.1 + 0.9 * used as f64 / n as f64,
        _ => 1.0,
    };
    let size = std::fs::metadata(&c.path).map_err(|e| e.to_string())?.len();
    let quant = int("general.file_type").map_or("Unknown", gguf::quant_name).to_string();
    let params_b = get("general.size_label")
        .and_then(Value::str)
        .and_then(size_label)
        .unwrap_or_else(|| size as f64 * 8.0 / gguf::bits_per_weight(&quant) / 1e9);
    let params_b = (params_b * 10.0).round() / 10.0;
    let name = c.name.clone().or_else(|| get("general.name").and_then(Value::str).map(str::to_string)).unwrap_or_else(|| describe(c));
    let hash = hex::encode(Sha256::digest(key(&c.path).as_bytes()));
    let vision = c.projector.as_ref().and_then(|p| {
        let size = std::fs::metadata(p).ok()?.len();
        Some(VisionFile { file: p.display().to_string(), url: String::new(), size, sha256: String::new() })
    });
    Ok(ModelSpec {
        id: format!("local-{}", &hash[..12]),
        name,
        publisher: c.app.to_string(),
        source: c.path.display().to_string(),
        description: format!("Downloaded by {}. SulcusAI uses that copy, so it takes no extra space.", c.app),
        license: License {
            name: "Its own license".into(),
            url: String::new(),
            commercial: true,
            note: Some("Downloaded by another app; check the model's license before using it for work.".into()),
        },
        tags: if vision.is_some() { vec!["vision".into()] } else { Vec::new() },
        params_b,
        arch: Arch { n_layer: n_layer as u32, n_kv_heads: kv_heads as u32, head_dim: head_dim as u32, max_ctx, active_fraction },
        default_ctx: 8192.min(max_ctx),
        tools: template.contains("tools"),
        variants: vec![Variant {
            quality: gguf::quality(&quant).into(),
            quant,
            file: c.path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            url: String::new(),
            size,
            sha256: String::new(),
        }],
        ratings: Ratings::default(),
        vision,
        local: Some(LocalSource { app: c.app.to_string(), path: c.path.display().to_string() }),
    })
}

#[derive(Debug, Default, Serialize)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

/// Adds the chat models other apps downloaded. `again` also brings back
/// ones the user had stopped using (they asked to look). Returns the names
/// added and the models that can't be used, with why.
pub fn adopt(state: &AppState, entries: &[Entry], again: bool) -> (Vec<String>, Vec<Skipped>) {
    let (mut known, hide, taken) = {
        let conn = state.db.lock().unwrap();
        if again {
            let _ = db::set(&conn, HIDDEN_KEY, &Vec::<String>::new());
        }
        let taken: Vec<String> = db::installed_models(&conn).iter().map(|m| key(Path::new(&m.path))).collect();
        (list(&conn), hidden(&conn), taken)
    };
    // Files that were deleted in the other app.
    let gone: Vec<ModelSpec> = known.iter().filter(|m| m.local.as_ref().is_some_and(|l| !Path::new(&l.path).exists())).cloned().collect();
    known.retain(|m| !gone.iter().any(|g| g.id == m.id));
    // Exact copies of catalog files are the catalog's to add.
    let catalog_file = |e: &Candidate| {
        let name = e.path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        let size = std::fs::metadata(&e.path).map_or(0, |m| m.len());
        state.catalog.models.iter().flat_map(|m| &m.variants).any(|v| v.size == size && v.file.to_lowercase() == name)
    };
    let budget = state.budget();
    let mut added: Vec<ModelSpec> = Vec::new();
    let mut fresh = Vec::new();
    let mut skipped = Vec::new();
    for c in candidates(entries) {
        let k = key(&c.path);
        let same = |m: &ModelSpec| m.local.as_ref().is_some_and(|l| key(Path::new(&l.path)) == k);
        // (Ollama can list one file under several names.)
        if known.iter().any(same) || fresh.iter().any(same) || hide.contains(&k) || taken.contains(&k) || catalog_file(&c) {
            continue;
        }
        match spec_for(&c) {
            Err(reason) => skipped.push(Skipped { name: describe(&c), reason }),
            Ok(spec) if !catalog::fit_model(&spec, &budget).variants.iter().any(|v| v.runnable()) => {
                skipped.push(Skipped { name: spec.name, reason: "needs more memory than this PC has".into() });
            }
            Ok(spec) => fresh.push(spec),
        }
    }
    if gone.is_empty() && fresh.is_empty() {
        return (Vec::new(), skipped);
    }
    {
        let conn = state.db.lock().unwrap();
        for g in &gone {
            let _ = db::remove_installed(&conn, &g.id);
        }
        for spec in &fresh {
            let v = &spec.variants[0];
            let saved = db::save_installed(&conn, &InstalledModel {
                model_id: spec.id.clone(),
                quant: v.quant.clone(),
                path: spec.local.as_ref().map(|l| l.path.clone()).unwrap_or_default(),
                size: v.size,
                installed_at: db::now_ms(),
                tps: None,
            });
            if saved.is_ok() {
                known.push(spec.clone());
                added.push(spec.clone());
            }
        }
        let _ = save(&conn, &known);
        let default = db::settings(&conn).default_model;
        let missing = default.as_ref().is_none_or(|d| gone.iter().any(|g| &g.id == d));
        if missing {
            let next = db::installed_models(&conn).first().map(|m| m.model_id.clone());
            let _ = db::update_settings(&conn, |s| s.default_model = next);
        }
    }
    for g in &gone {
        state.log("model", &format!("{} was deleted in {}, so it's no longer listed", g.name, g.publisher));
    }
    for spec in &added {
        state.log("model", &format!("Found {} in {} and added it (SulcusAI uses {}'s copy, so it takes no extra space)", spec.name, spec.publisher, spec.publisher));
    }
    (added.into_iter().map(|s| s.name).collect(), skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_size_labels() {
        assert_eq!(size_label("14B"), Some(14.0));
        assert_eq!(size_label("1.5B"), Some(1.5));
        assert_eq!(size_label("30B-A3B"), Some(30.0));
        assert_eq!(size_label("8x7B"), Some(56.0));
        assert_eq!(size_label("500M"), Some(0.5));
        assert_eq!(size_label("large"), None);
    }

    #[test]
    fn names_ollama_models_from_their_manifests() {
        let root = std::env::temp_dir().join(format!("sulcusai-ollama-{}", uuid::Uuid::new_v4()));
        let m = root.join("manifests/registry.ollama.ai/library/qwen2.5-coder");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("14b"),
            r#"{"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:abc"},{"mediaType":"application/vnd.ollama.image.template","digest":"sha256:def"}]}"#,
        )
        .unwrap();
        let found = ollama(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name.as_deref(), Some("qwen2.5-coder:14b"));
        assert_eq!(found[0].path, root.join("blobs").join("sha256-abc"));
        assert!(found[0].projector.is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}

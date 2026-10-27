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
        // A model split into parts is listed once, by its first part.
        if e.app == "Ollama" || !e.name.ends_with(".gguf") || e.name.starts_with("mmproj") || e.size < MIN_SIZE || later_part(&e.name.to_lowercase()) {
            continue;
        }
        out.push(Candidate { app: e.app, path: e.path.clone(), name: None, projector: projector_beside(&e.path) });
    }
    out
}

/// LM Studio and Hugging Face keep the picture encoder beside the model.
/// With several in one folder, the one whose output matches the model's
/// width (a 4B's encoder doesn't fit an 8B).
fn projector_beside(model: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(model.parent()?)
        .ok()?
        .flatten()
        .map(|f| f.path())
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            n.starts_with("mmproj") && n.ends_with(".gguf")
        })
        .collect();
    found.sort();
    if found.len() > 1 {
        let width = gguf::read(model).ok().and_then(|m| {
            let arch = m.get("general.architecture").and_then(Value::str)?.to_string();
            m.get(&format!("{arch}.embedding_length")).and_then(Value::int)
        });
        if let Some(w) = width {
            if let Some(p) = found.iter().find(|p| gguf::read(p).ok().and_then(|m| m.get("clip.vision.projection_dim").and_then(Value::int)) == Some(w)) {
                return Some(p.clone());
            }
        }
    }
    found.into_iter().next()
}

/// The size of a model on disk: all its parts when it's split
/// ("…-00001-of-00003.gguf").
fn size_with_parts(path: &Path) -> u64 {
    let own = std::fs::metadata(path).map_or(0, |m| m.len());
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    let stem = name.trim_end_matches(".gguf");
    let mut it = stem.rsplitn(4, '-');
    let (Some(total), Some("of"), Some(part), Some(base)) = (it.next(), it.next(), it.next(), it.next()) else { return own };
    if part.trim_start_matches('0') != "1" || !total.bytes().all(|b| b.is_ascii_digit()) {
        return own;
    }
    let Some(dir) = path.parent() else { return own };
    let prefix = format!("{base}-");
    let suffix = format!("-of-{total}.gguf");
    std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    n.starts_with(&prefix) && n.ends_with(&suffix)
                })
                .filter_map(|e| e.metadata().ok().map(|m| m.len()))
                .sum::<u64>()
        })
        .unwrap_or(own)
        .max(own)
}

/// What a model file is, from its header (for a download, once it's here).
pub fn read_spec(path: &Path) -> Result<ModelSpec, String> {
    spec_for(&Candidate { app: "Hugging Face", path: path.to_path_buf(), name: None, projector: projector_beside(path) })
}

/// Adds a model file the user picked (Settings → Advanced). Returns its name.
pub fn import(state: &AppState, path: &Path) -> Result<String, String> {
    let k = key(path);
    let taken = {
        let conn = state.db.lock().unwrap();
        list(&conn).iter().any(|m| m.local.as_ref().is_some_and(|l| key(Path::new(&l.path)) == k))
            || db::installed_models(&conn).iter().any(|m| key(Path::new(&m.path)) == k)
    };
    if taken {
        return Err("That model is already in your list.".into());
    }
    let c = Candidate { app: "Imported", path: path.to_path_buf(), name: None, projector: projector_beside(path) };
    let spec = spec_for(&c).map_err(|reason| format!("That file {reason}."))?;
    if !catalog::fit_model(&spec, &state.budget()).variants.iter().any(|v| v.runnable()) {
        return Err("That model needs more memory than this PC has.".into());
    }
    let v = &spec.variants[0];
    {
        let conn = state.db.lock().unwrap();
        db::save_installed(&conn, &InstalledModel { model_id: spec.id.clone(), quant: v.quant.clone(), path: c.path.display().to_string(), size: v.size, installed_at: db::now_ms(), tps: None , prompt_tps: None})?;
        let mut all = list(&conn);
        all.push(spec.clone());
        save(&conn, &all)?;
        let mut h = hidden(&conn);
        h.retain(|p| p != &k);
        db::set(&conn, HIDDEN_KEY, &h)?;
        if db::settings(&conn).default_model.is_none() {
            db::update_settings(&conn, |s| s.default_model = Some(spec.id.clone()))?;
        }
    }
    state.log("model", &format!("Imported {} from {} (used where it is, not copied)", spec.name, c.path.display()));
    Ok(spec.name)
}

/// What adding a folder did.
#[derive(Debug, Default, Serialize)]
pub struct FolderImport {
    /// Names of the models added.
    pub added: Vec<String>,
    /// Model files that weren't added, with why (a whole sentence).
    pub skipped: Vec<Skipped>,
}

/// Adds the chat models in a folder the user picked or typed (its
/// subfolders too), or a single .gguf file; used where they are.
pub fn import_folder(state: &AppState, path: &Path) -> Result<FolderImport, String> {
    if path.is_file() {
        return import(state, path).map(|name| FolderImport { added: vec![name], skipped: Vec::new() });
    }
    if !path.is_dir() {
        return Err(format!("There's no folder or file at {}.", path.display()));
    }
    let files = gguf_files(path, MIN_SIZE);
    if files.is_empty() {
        return Err(if has_file(path, ".safetensors", 3) {
            "This folder only has the model's original weights (.safetensors), which SulcusAI's engine can't run. It needs a GGUF version of the model; the model's page often offers one.".into()
        } else {
            "There's no model file (.gguf) in this folder or its subfolders.".into()
        });
    }
    let mut r = FolderImport::default();
    for f in files {
        match import(state, &f) {
            Ok(name) => r.added.push(name),
            Err(reason) => r.skipped.push(Skipped { name: f.file_name().unwrap_or_default().to_string_lossy().into_owned(), reason }),
        }
    }
    Ok(r)
}

/// The model files in a folder and its subfolders: .gguf files big enough to
/// be models, leaving out picture encoders and the later parts of a split
/// model (llama.cpp loads those from the first part). Download caches and
/// other hidden folders are skipped.
fn gguf_files(dir: &Path, min_size: u64) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, min: u64, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if depth > 0 && !name.starts_with('.') {
                    walk(&e.path(), depth - 1, min, out);
                }
            } else if name.ends_with(".gguf") && !name.starts_with("mmproj") && !later_part(&name) && e.metadata().is_ok_and(|m| m.len() >= min) && out.len() < 50 {
                out.push(e.path());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 4, min_size, &mut out);
    out
}

/// How a model keeps its conversation memory, from its header: which layers
/// keep the whole window, which only a sliding window, and how wide each
/// is. Read from what the file lists (Gemma 4: which layers slide, how many
/// share another's memory, K/V heads per layer), else the family's pattern
/// (Gemma 3, gpt-oss), else Qwen3.5's one-in-N full-attention layers.
/// Returns the memory fields of an `Arch` (the rest left at zero).
fn kv_layout(meta: &gguf::Meta, arch: &str, n_layer: usize, heads: i64, head_dim: i64) -> Arch {
    let get = |k: &str| meta.get(&format!("{arch}.{k}"));
    let per_layer = |k: &str| -> Vec<i64> {
        match get(k) {
            Some(gguf::Value::Ints(v)) if v.len() == n_layer => v.clone(),
            Some(v) => vec![v.int().unwrap_or(0); n_layer],
            None => Vec::new(),
        }
    };
    let kv_heads = per_layer("attention.head_count_kv");
    let kv_at = |i: usize| kv_heads.get(i).copied().filter(|h| *h > 0).unwrap_or(heads);
    let int = |k: &str| get(k).and_then(gguf::Value::int);
    let window = int("attention.sliding_window").unwrap_or(0).max(0) as u32;
    let shared = (int("attention.shared_kv_layers").unwrap_or(0).max(0) as usize).min(n_layer);
    let own = n_layer - shared;
    // Which of the layers with their own memory slide (yes = 1).
    let pattern = per_layer("attention.sliding_window_pattern");
    let every = crate::catalog::swa_every(arch) as usize;
    let sliding: Vec<bool> = if window > 0 && pattern.len() == n_layer {
        pattern[..own].iter().map(|p| *p != 0).collect()
    } else if window > 0 && every > 0 {
        // llama.cpp: layer i keeps everything when i % every == every - 1.
        (0..own).map(|i| i % every != every - 1).collect()
    } else {
        Vec::new()
    };
    let interval = int("full_attention_interval").unwrap_or(0).max(0) as usize;
    let mut a = Arch { n_layer: n_layer as u32, n_kv_heads: kv_at(0) as u32, head_dim: head_dim as u32, max_ctx: 0, active_fraction: 1.0, kv_whole: 0, kv_sliding: 0, kv_whole_width: 0, swa_window: 0 };
    if !sliding.is_empty() {
        let first_whole = sliding.iter().position(|s| !s);
        let first_sliding = sliding.iter().position(|s| *s);
        a.kv_whole = sliding.iter().filter(|s| !**s).count() as u32;
        a.kv_sliding = own as u32 - a.kv_whole;
        a.swa_window = window;
        // The sliding layers' shape is the main one; the whole-window
        // layers' is kept beside it when it differs.
        if let Some(i) = first_sliding {
            a.n_kv_heads = kv_at(i) as u32;
            a.head_dim = int("attention.key_length_swa").unwrap_or(head_dim) as u32;
        }
        if let Some(i) = first_whole {
            let width = (kv_at(i) * head_dim) as u32;
            if width != a.n_kv_heads * a.head_dim {
                a.kv_whole_width = width;
            }
        }
    } else if interval > 0 {
        // Linear-attention layers keep a small fixed state, not the window.
        a.kv_whole = (n_layer / interval) as u32;
    } else if shared > 0 {
        a.kv_whole = own as u32;
    }
    a
}

/// Deletes a model file and, when it's the first part of a split model, its
/// other parts too.
pub fn remove_with_parts(path: &Path) {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    let stem = name.trim_end_matches(".gguf");
    let mut it = stem.rsplitn(4, '-');
    if let (Some(total), Some("of"), Some(part), Some(base), Some(dir)) = (it.next(), it.next(), it.next(), it.next(), path.parent()) {
        if part.trim_start_matches('0') == "1" && total.bytes().all(|b| b.is_ascii_digit()) {
            let (prefix, suffix) = (format!("{base}-"), format!("-of-{total}.gguf"));
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if n.starts_with(&prefix) && n.ends_with(&suffix) && later_part(&n) {
                    std::fs::remove_file(e.path()).ok();
                }
            }
        }
    }
    std::fs::remove_file(path).ok();
}

/// "…-00002-of-00003.gguf": the second or a later part of a split model.
fn later_part(name: &str) -> bool {
    let stem = name.trim_end_matches(".gguf");
    let mut it = stem.rsplit('-');
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    matches!((it.next(), it.next(), it.next()), (Some(total), Some("of"), Some(part)) if digits(total) && digits(part) && part.trim_start_matches('0') != "1")
}

/// A file ending in `ext` somewhere within `depth` levels of `dir`.
fn has_file(dir: &Path, ext: &str, depth: usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_lowercase();
        match e.file_type() {
            Ok(t) if t.is_dir() => depth > 0 && !name.starts_with('.') && has_file(&e.path(), ext, depth - 1),
            Ok(_) => name.ends_with(ext),
            Err(_) => false,
        }
    })
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
/// A model another app downloaded rates like the same model in SulcusAI's
/// catalog: "qwen2.5-coder:14b" like qwen2.5-coder-7b (the same family), and
/// "gpt-oss:20b" like gpt-oss-20b. Unknown families get no ratings.
fn known_ratings(name: &str, params_b: f64) -> Ratings {
    static CATALOG: std::sync::LazyLock<crate::catalog::Catalog> = std::sync::LazyLock::new(crate::catalog::Catalog::bundled);
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).to_lowercase().replace(':', "-");
    let base = base.trim_end_matches(".gguf").trim_end_matches("-latest").replace("-instruct", "");
    let size = |id: &str| id.rsplit('-').find_map(|p| p.strip_suffix('b')?.parse::<f64>().ok());
    let family = |id: &str| id.split('-').take_while(|p| !(p.ends_with('b') && p[..p.len() - 1].parse::<f64>().is_ok())).collect::<Vec<_>>().join("-");
    let models = &CATALOG.models;
    if let Some(m) = models.iter().find(|m| m.id == base || m.id.starts_with(&format!("{base}-"))) {
        return m.ratings.clone();
    }
    // Other apps spell the same model differently: Ollama's "gemma3:12b"
    // is the catalog's "gemma-3-12b", "qwen3.5:9b" its "qwen3.5-9b".
    let bare = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    if let Some(m) = models.iter().find(|m| bare(&m.id) == bare(&base)) {
        return m.ratings.clone();
    }
    let fam = family(&base);
    if fam.is_empty() {
        return Ratings::default();
    }
    models
        .iter()
        .filter(|m| family(&m.id) == fam)
        .min_by(|a, b| {
            let gap = |m: &&crate::catalog::ModelSpec| (size(&m.id).unwrap_or(m.params_b) - params_b).abs();
            gap(a).total_cmp(&gap(b))
        })
        .map(|m| m.ratings.clone())
        .unwrap_or_default()
}

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
    let head_dim = a("attention.key_length").or_else(|| a("embedding_length").map(|e| e / heads)).unwrap_or(128);
    // Up to 1M tokens: the window sizes past what this PC holds show as too much.
    let max_ctx = a("context_length").unwrap_or(4096).clamp(2048, 1 << 20) as u32;
    let active_fraction = match (a("expert_count"), a("expert_used_count")) {
        // The experts hold most of the weights; only a few run per token.
        (Some(n), Some(used)) if n > 0 => 0.1 + 0.9 * used as f64 / n as f64,
        _ => 1.0,
    };
    let memory = kv_layout(&meta, &arch, n_layer.max(1) as usize, heads, head_dim);
    std::fs::metadata(&c.path).map_err(|e| e.to_string())?;
    let size = size_with_parts(&c.path);
    let header = int("general.file_type").map_or("Unknown", gguf::quant_name);
    // A header saying full precision over quantized weights is a converter's
    // slip; the weights themselves say what they are.
    let quant = match (header, int(gguf::MAIN_TYPE_KEY).and_then(gguf::tensor_quant_name)) {
        ("F32" | "F16" | "BF16" | "Unknown", Some(real)) => real,
        _ => header,
    }
    .to_string();
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
        arch: Arch { n_layer: n_layer as u32, max_ctx, active_fraction, ..memory },
        default_ctx: 8192.min(max_ctx),
        tools: template.contains("tools"),
        variants: vec![Variant {
            quality: gguf::quality(&quant).into(),
            quant,
            file: c.path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            url: String::new(),
            size,
            sha256: String::new(),
            parts: Vec::new(),
            mtp: false,
        }],
        ratings: known_ratings(c.name.as_deref().unwrap_or(&c.path.display().to_string()), params_b),
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
                tps: None, prompt_tps: None,
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

    /// Conversation memory as llama.cpp b11450 allocated it at 16K, from
    /// the real headers: Qwen3.5 0.8B keeps it in 6 of 24 layers (192 MiB),
    /// Gemma 4 E2B in 3 whole-window and 12 sliding layers, 20 sharing
    /// (96 + 12 MiB), Gemma 3 4B in 5 whole and 29 sliding (320 + 174 MiB).
    #[test]
    fn layers_plan_their_conversation_memory_as_the_engine_does() {
        use gguf::Value::{Int, Ints};
        let mib = |a: &Arch| crate::catalog::kv_cache_bytes(a, 16384) / (1024 * 1024);
        let meta = |pairs: Vec<(&str, gguf::Value)>| pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<gguf::Meta>();

        let qwen = meta(vec![("qwen35.attention.head_count_kv", Int(2)), ("qwen35.full_attention_interval", Int(4))]);
        let a = kv_layout(&qwen, "qwen35", 24, 8, 256);
        assert_eq!((a.kv_whole, a.kv_sliding, mib(&a)), (6, 0, 192));

        let pattern: Vec<i64> = (0..35).map(|i| (i % 5 != 4) as i64).collect();
        let gemma4 = meta(vec![
            ("gemma4.attention.head_count_kv", Int(1)),
            ("gemma4.attention.key_length_swa", Int(256)),
            ("gemma4.attention.sliding_window", Int(512)),
            ("gemma4.attention.shared_kv_layers", Int(20)),
            ("gemma4.attention.sliding_window_pattern", Ints(pattern)),
        ]);
        let a = kv_layout(&gemma4, "gemma4", 35, 8, 512);
        assert_eq!((a.kv_whole, a.kv_sliding, a.kv_whole_width, mib(&a)), (3, 12, 512, 96 + 12));

        let gemma3 = meta(vec![("gemma3.attention.head_count_kv", Int(4)), ("gemma3.attention.sliding_window", Int(1024))]);
        let a = kv_layout(&gemma3, "gemma3", 34, 8, 256);
        assert_eq!((a.kv_whole, a.kv_sliding, mib(&a)), (5, 29, 320 + 174));

        let plain = meta(vec![("qwen3.attention.head_count_kv", Int(8))]);
        let a = kv_layout(&plain, "qwen3", 36, 32, 128);
        assert_eq!((a.kv_whole, a.kv_sliding), (0, 0), "every layer keeps the whole window");
    }

    #[test]
    fn a_split_model_is_as_big_as_all_its_parts() {
        let dir = std::env::temp_dir().join(format!("sulcus-split-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, len) in [("big-Q4_K_M-00001-of-00003.gguf", 10), ("big-Q4_K_M-00002-of-00003.gguf", 20), ("big-Q4_K_M-00003-of-00003.gguf", 5), ("other.gguf", 99)] {
            std::fs::write(dir.join(name), vec![0u8; len]).unwrap();
        }
        assert_eq!(size_with_parts(&dir.join("big-Q4_K_M-00001-of-00003.gguf")), 35);
        assert_eq!(size_with_parts(&dir.join("other.gguf")), 99);
        // Replacing it with another version deletes every part, not just the first.
        remove_with_parts(&dir.join("big-Q4_K_M-00001-of-00003.gguf"));
        let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["other.gguf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn models_from_other_apps_rate_like_their_catalog_twins() {
        assert_eq!(known_ratings("gpt-oss:20b", 20.9).agents, 9);
        assert_eq!(known_ratings("qwen3-coder:30b", 30.5).agents, 8);
        assert_eq!(known_ratings("qwen2.5-coder:14b", 14.8).agents, 4, "same family, nearest size");
        assert_eq!(known_ratings("qwen3:8b", 8.2).agents, 7);
        assert_eq!(known_ratings("gemma3:12b", 12.0).writing, 8, "Ollama leaves out the catalog's dashes");
        assert_eq!(known_ratings("qwen3.5:9b", 9.0).overall, 74);
        assert_eq!(known_ratings(r"C:\models\qwen3-14b-Q4_K_M.gguf", 14.8).agents, 8);
        assert_eq!(known_ratings("some-new-model:7b", 7.0).agents, 0, "unknown: no ratings");
    }

    #[test]
    fn a_folder_offers_its_model_files_but_not_caches_encoders_or_later_parts() {
        let dir = std::env::temp_dir().join(format!("sulcusai-folder-{}", uuid::Uuid::new_v4().simple()));
        let write = |rel: &str, len: usize| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![0u8; len]).unwrap();
        };
        write("model-00001-of-00003.safetensors", 10);
        write("GGUF/Big-20b-mxfp4.gguf", 100);
        write("GGUF/mmproj-Big-f16.gguf", 100);
        write("split/Huge-Q4_K_M-00001-of-00002.gguf", 100);
        write("split/Huge-Q4_K_M-00002-of-00002.gguf", 100);
        write("tiny.gguf", 5);
        write(".cache/huggingface/download/GGUF/Big-20b-mxfp4.gguf", 100);
        let names: Vec<String> = gguf_files(&dir, 50).iter().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, ["GGUF/Big-20b-mxfp4.gguf", "split/Huge-Q4_K_M-00001-of-00002.gguf"]);
        assert!(has_file(&dir, ".safetensors", 3));
        assert!(!later_part("qwen3-8b-q4_k_m.gguf") && !later_part("a-00001-of-00002.gguf") && later_part("a-00010-of-00012.gguf"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A real model folder (SULCUSAI_TEST_MODEL_DIR), e.g. a Hugging Face
    /// download with the .gguf in a GGUF subfolder.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn adds_the_models_in_a_real_folder() {
        let dir = std::env::var("SULCUSAI_TEST_MODEL_DIR").expect("SULCUSAI_TEST_MODEL_DIR");
        let state = crate::e2e::scratch_state();
        let r = import_folder(&state, Path::new(&dir)).unwrap();
        println!("added {:?}, skipped {:?}", r.added, r.skipped);
        assert!(!r.added.is_empty());
        let spec = list(&state.db.lock().unwrap()).into_iter().find(|m| m.name == r.added[0]).unwrap();
        println!("{} · {} · {}B · ctx {} · tools {} · {:?}", spec.name, spec.variants[0].quant, spec.params_b, spec.default_ctx, spec.tools, spec.arch);
        assert!(import_folder(&state, Path::new(&dir)).unwrap().added.is_empty(), "adding it again changes nothing");
    }

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

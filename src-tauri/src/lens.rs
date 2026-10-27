// SPDX-License-Identifier: AGPL-3.0-only
//! The Lens: looking inside a model on this PC.
//!
//! - Words: a reply, each piece with how sure the model was and what else
//!   it weighed; picking another piece carries the reply on from there.
//! - Layers: which layers, and which part of each (attention, feed-forward),
//!   a text used most, from llama.cpp's own activation statistics
//!   (`llama-imatrix`, the numbers it uses to quantize well), and the
//!   feed-forward units ("neurons") it leaned on hardest compared with
//!   everyday text.
//! - Edits: a copy of the model with chosen neurons turned up, down or off
//!   (each neuron's row of `ffn_up` scaled, which scales its output
//!   exactly), added to the models list to compare replies. The original
//!   is never changed.
//!
//! Every part can fail on its own (a cloud model, a model built from
//! experts, an unusual weight format): the others still work, and the
//! person is told why in plain words.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{db, engine, gguf, net, AppState, AppStateRef};

// ---------- words ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Alt {
    pub text: String,
    pub prob: f32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Piece {
    pub text: String,
    /// How sure the model was of this piece (0..1).
    pub prob: f32,
    /// What else it weighed, most likely first (this piece among them).
    pub alts: Vec<Alt>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Words {
    pub model: String,
    /// The part of the reply that was given (a branch's start), then the
    /// pieces the model chose.
    pub given: String,
    pub pieces: Vec<Piece>,
}

/// One piece from llama-server's `completion_probabilities` (both the
/// newer `top_logprobs` form and the older `probs` one).
pub fn piece_of(v: &Value) -> Option<Piece> {
    let text = v.get("token").or_else(|| v.get("content")).and_then(Value::as_str)?.to_string();
    let prob_of = |x: &Value| -> Option<f32> {
        x.get("logprob").and_then(Value::as_f64).map(|l| l.exp() as f32).or_else(|| x.get("prob").and_then(Value::as_f64).map(|p| p as f32))
    };
    let alts: Vec<Alt> = v
        .get("top_logprobs")
        .or_else(|| v.get("top_probs"))
        .or_else(|| v.get("probs"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| Some(Alt { text: x.get("token").or_else(|| x.get("tok_str")).and_then(Value::as_str)?.to_string(), prob: prob_of(x)? }))
                .collect()
        })
        .unwrap_or_default();
    let prob = prob_of(v).or_else(|| alts.iter().find(|a| a.text == text).map(|a| a.prob)).unwrap_or(0.0);
    Some(Piece { text, prob, alts })
}

/// The chat template applied to the prompt (so the model sees what it
/// sees in a chat), else the prompt as it is.
async fn templated(ep: &engine::Endpoint, prompt: &str) -> String {
    let body = json!({ "messages": [{ "role": "user", "content": prompt }], "chat_template_kwargs": { "enable_thinking": false } });
    let r = net::local_client().post(ep.url("/apply-template")).bearer_auth(&ep.key).json(&body).timeout(Duration::from_secs(30)).send().await;
    match r {
        Ok(r) => r.json::<Value>().await.ok().and_then(|v| v.get("prompt").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| prompt.to_string()),
        Err(_) => prompt.to_string(),
    }
}

/// A reply to `prompt` with each piece's odds, starting from `given` (the
/// start of a reply to carry on from); the model's most likely choice at
/// each step, so a change is the edit's doing, not chance.
pub async fn words(state: &Arc<AppState>, model_id: Option<&str>, prompt: &str, given: &str, top: u32, most: u32) -> Result<Words, String> {
    let (ep, spec, _hold) = crate::endpoint_for(state, model_id).await?;
    if ep.cloud.is_some() {
        return Err("The Lens looks inside models on this PC; a cloud model doesn't share what's inside it. Pick a model on this PC.".into());
    }
    let text = templated(&ep, prompt).await + given;
    let body = json!({ "prompt": text, "n_predict": most.clamp(8, 1024), "n_probs": top.clamp(2, 10), "temperature": 0.0, "cache_prompt": true, "stream": false });
    let v: Value = net::local_client()
        .post(ep.url("/completion"))
        .bearer_auth(&ep.key)
        .json(&body)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the model: {e}"))?
        .json()
        .await
        .map_err(|e| format!("The model sent an unreadable answer: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(format!("The model couldn't answer: {}", e.get("message").and_then(Value::as_str).unwrap_or("unknown error")));
    }
    let mut pieces: Vec<Piece> = v.get("completion_probabilities").and_then(Value::as_array).map(|a| a.iter().filter_map(piece_of).collect()).unwrap_or_default();
    if pieces.is_empty() {
        // An engine without odds: the reply as one piece.
        let content = v.get("content").and_then(Value::as_str).unwrap_or_default();
        pieces.push(Piece { text: content.to_string(), prob: 0.0, alts: Vec::new() });
    }
    Ok(Words { model: spec.name, given: given.to_string(), pieces })
}

// ---------- layers ----------

/// The part of a layer a weight matrix's input belongs to, from its name.
pub fn part_of(name: &str) -> Option<(u32, &'static str)> {
    let rest = name.strip_prefix("blk.")?;
    let (layer, tail) = rest.split_once('.')?;
    let layer: u32 = layer.parse().ok()?;
    let part = if tail.starts_with("attn_q") || tail.starts_with("attn_qkv") {
        "attention_in"
    } else if tail.starts_with("attn_output") || tail.starts_with("attn_out") {
        "attention_out"
    } else if tail.starts_with("ffn_up") || tail.starts_with("ffn_gate") && !tail.starts_with("ffn_gate_inp") {
        "ffn_in"
    } else if tail.starts_with("ffn_down") {
        "ffn_inside"
    } else {
        return None;
    };
    Some((layer, part))
}

/// The parts, in the order the map shows them.
pub const PARTS: [&str; 4] = ["attention_in", "attention_out", "ffn_in", "ffn_inside"];

/// llama-imatrix's legacy file: per weight matrix, its name and the mean
/// square of each input channel over the text.
pub fn read_imatrix(bytes: &[u8]) -> Result<Vec<(String, Vec<f32>)>, String> {
    let mut at = 0usize;
    let i32_at = |at: &mut usize| -> Result<i32, String> {
        let b = bytes.get(*at..*at + 4).ok_or("The statistics file is cut short.")?;
        *at += 4;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let n = i32_at(&mut at)?;
    if !(0..=200_000).contains(&n) {
        return Err("The statistics file isn't in the expected form.".into());
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let len = i32_at(&mut at)?;
        if !(0..=4096).contains(&len) {
            return Err("The statistics file isn't in the expected form.".into());
        }
        let name = String::from_utf8_lossy(bytes.get(at..at + len as usize).ok_or("The statistics file is cut short.")?).into_owned();
        at += len as usize;
        let calls = i32_at(&mut at)?.max(1) as f32;
        let nval = i32_at(&mut at)?;
        if !(0..=50_000_000).contains(&nval) {
            return Err("The statistics file isn't in the expected form.".into());
        }
        let raw = bytes.get(at..at + 4 * nval as usize).ok_or("The statistics file is cut short.")?;
        at += 4 * nval as usize;
        out.push((name, raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) / calls).collect()));
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Neuron {
    pub layer: u32,
    pub index: u32,
    /// How strongly it fired for the text.
    pub energy: f32,
    /// How many times more than for everyday text.
    pub ratio: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerMap {
    pub layers: u32,
    pub parts: Vec<&'static str>,
    /// energy[layer][part]: how strongly that part was used (mean square of its inputs).
    pub energy: Vec<Vec<f32>>,
    /// The same for everyday text, to compare against.
    pub usual: Vec<Vec<f32>>,
    /// The neurons this text used much more than everyday text does.
    pub neurons: Vec<Neuron>,
    /// Whether neurons can be edited in this model (feed-forward not split into experts, a weight format the Lens can scale).
    pub editable: Option<String>,
}

/// Everyday text to compare against: what a model reads all the time.
const USUAL: &str = "The town library opens at nine in the morning and closes at six. On weekdays, people come in to borrow books, read the newspaper, or use the computers. \
Children's story time is on Saturday mornings, and there is a small cafe near the entrance that sells coffee, tea and sandwiches. Last year the library added a garden \
with benches where visitors can sit and read when the weather is good. Volunteers help sort returned books and answer questions about local history.";

fn per_part(stats: &[(String, Vec<f32>)], layers: u32) -> Vec<Vec<f32>> {
    let mut sum = vec![vec![0f64; PARTS.len()]; layers as usize];
    let mut n = vec![vec![0u32; PARTS.len()]; layers as usize];
    for (name, values) in stats {
        let Some((l, part)) = part_of(name) else { continue };
        let Some(p) = PARTS.iter().position(|x| *x == part) else { continue };
        if let (Some(s), Some(c)) = (sum.get_mut(l as usize), n.get_mut(l as usize)) {
            s[p] += values.iter().map(|v| *v as f64).sum::<f64>() / values.len().max(1) as f64;
            c[p] += 1;
        }
    }
    sum.iter().zip(&n).map(|(s, c)| s.iter().zip(c).map(|(v, k)| if *k > 0 { (*v / *k as f64) as f32 } else { 0.0 }).collect()).collect()
}

/// The feed-forward units the text used most beyond the everyday ones.
pub fn standouts(text: &[(String, Vec<f32>)], usual: &[(String, Vec<f32>)], most: usize) -> Vec<Neuron> {
    let mut out = Vec::new();
    for (name, values) in text {
        let Some((layer, "ffn_inside")) = part_of(name) else { continue };
        // Units split among experts are numbered per expert: left out.
        if name.contains("exps") {
            continue;
        }
        let Some((_, base)) = usual.iter().find(|(n, _)| n == name) else { continue };
        if base.len() != values.len() || values.is_empty() {
            continue;
        }
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        // Against the unit's own everyday level, but never less than the
        // layer's typical unit: units nearly silent on everyday text all
        // came out "over 100 times", which said nothing.
        let floor = base.iter().sum::<f32>() / base.len() as f32 + 1e-9;
        for (i, (v, b)) in values.iter().zip(base).enumerate() {
            // Strong for this text, and well above what everyday text gives it.
            if *v > mean * 2.0 {
                out.push(Neuron { layer, index: i as u32, energy: *v, ratio: *v / b.max(floor) });
            }
        }
    }
    out.sort_by(|a, b| (b.ratio * b.energy.sqrt()).total_cmp(&(a.ratio * a.energy.sqrt())));
    out.truncate(most);
    out
}

/// The model's file, for one on this PC.
fn gguf_of(state: &AppState, id: &str) -> Option<PathBuf> {
    let conn = state.db.lock().unwrap();
    if let Some(m) = db::installed_models(&conn).into_iter().find(|m| m.model_id == id) {
        return Some(crate::model_file(&state.paths, &m));
    }
    crate::local::list(&conn).into_iter().find(|m| m.id == id).and_then(|m| m.local.map(|l| PathBuf::from(l.path)))
}

/// The model the Lens looks at: the one asked for, else the default.
fn model_for(state: &AppState, model_id: Option<&str>) -> Result<(String, PathBuf), String> {
    let id = model_id.filter(|m| !m.is_empty()).map(str::to_string).or_else(|| db::settings(&state.db.lock().unwrap()).default_model).ok_or("Pick a model on this PC first.")?;
    let path = gguf_of(state, &id).ok_or("The Lens looks inside models on this PC; that one isn't (or its file is missing).")?;
    Ok((id, path))
}

/// llama.cpp's statistics for `text` read by the model at `gguf`, on the
/// processor (so a chat on the card isn't disturbed).
async fn statistics(state: &Arc<AppState>, gguf: &Path, text: &str) -> Result<Vec<(String, Vec<f32>)>, String> {
    let backend = engine::backend_for(&state.budget()).unwrap_or("cpu");
    let server = engine::installed_server(&state.paths, &state.catalog.engine, backend)
        .or_else(|| engine::installed_server(&state.paths, &state.catalog.engine, "cpu"))
        .ok_or("The model engine isn't installed.")?;
    let exe = server.with_file_name(if cfg!(windows) { "llama-imatrix.exe" } else { "llama-imatrix" });
    if !exe.is_file() {
        return Err("This model engine doesn't include llama.cpp's statistics tool, so the layer map isn't available.".into());
    }
    let dir = std::env::temp_dir().join(format!("sulcusai-lens-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // The tool reads the text in windows of 128 pieces and needs at least
    // two: a short text is repeated (the same statistics, more of them).
    let mut body = text.trim().to_string();
    while body.len() < 1600 {
        body.push_str("\n\n");
        body.push_str(text.trim());
    }
    std::fs::write(dir.join("text.txt"), &body).map_err(|e| e.to_string())?;
    let out = dir.join("stats.dat");
    let threads = state.limits().threads.max(2);
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("-m")
        .arg(gguf)
        .arg("-f")
        .arg(dir.join("text.txt"))
        .arg("-o")
        .arg(&out)
        .args(["--output-format", "dat", "-c", "128", "-ngl", "0", "-t", &threads.to_string(), "--no-ppl", "--chunks", "16"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    engine::tie_to_app(&mut cmd);
    let ran = tokio::time::timeout(Duration::from_secs(900), cmd.output()).await;
    let bytes = std::fs::read(&out);
    let _ = std::fs::remove_dir_all(&dir);
    let ran = ran.map_err(|_| "Reading the model's layers took too long.".to_string())?.map_err(|e| format!("Couldn't start llama.cpp's statistics tool: {e}"))?;
    let bytes = bytes.map_err(|_| {
        let err = String::from_utf8_lossy(&ran.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no reason given");
        format!("llama.cpp's statistics tool didn't finish ({last}).")
    })?;
    read_imatrix(&bytes)
}

/// Statistics for everyday text, kept per model file (they don't change).
async fn usual_statistics(state: &Arc<AppState>, gguf: &Path) -> Result<Vec<(String, Vec<f32>)>, String> {
    let key = format!("{:x}", md5_like(&gguf.display().to_string()));
    let cache = state.paths.data.join("lens").join(format!("{key}.dat"));
    if let Ok(b) = std::fs::read(&cache) {
        if let Ok(s) = read_imatrix(&b) {
            return Ok(s);
        }
    }
    let s = statistics(state, gguf, USUAL).await?;
    let _ = std::fs::create_dir_all(cache.parent().unwrap_or(Path::new(".")));
    let _ = std::fs::write(&cache, write_imatrix(&s));
    Ok(s)
}

/// A short stable number for a file's path (the cache's file name).
fn md5_like(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

fn write_imatrix(s: &[(String, Vec<f32>)]) -> Vec<u8> {
    let mut out = (s.len() as i32).to_le_bytes().to_vec();
    for (name, values) in s {
        out.extend((name.len() as i32).to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(1i32.to_le_bytes());
        out.extend((values.len() as i32).to_le_bytes());
        for v in values {
            out.extend(v.to_le_bytes());
        }
    }
    out
}

pub async fn layers(state: &Arc<AppState>, model_id: Option<&str>, text: &str) -> Result<LayerMap, String> {
    let (_, path) = model_for(state, model_id)?;
    let meta = gguf::read(&path).map_err(|e| format!("Couldn't read the model's file: {e}"))?;
    let arch = meta.get("general.architecture").and_then(gguf::Value::str).unwrap_or("llama").to_string();
    let layers = meta.get(&format!("{arch}.block_count")).and_then(gguf::Value::int).unwrap_or(0).max(0) as u32;
    let usual = usual_statistics(state, &path).await?;
    let mine = statistics(state, &path, text).await?;
    let layers = if layers > 0 { layers } else { mine.iter().filter_map(|(n, _)| part_of(n).map(|(l, _)| l + 1)).max().unwrap_or(0) };
    let editable = tokio::task::spawn_blocking(move || why_not_editable(&path)).await.map_err(|e| e.to_string())?;
    Ok(LayerMap { layers, parts: PARTS.to_vec(), energy: per_part(&mine, layers), usual: per_part(&usual, layers), neurons: standouts(&mine, &usual, 24), editable })
}

// ---------- edits ----------

/// How a row of a weight type is scaled in place: values a block, bytes a
/// block, and where in each block its 16-bit scales are (scaling those
/// scales every value in the block exactly).
pub fn block_scales(kind: u32) -> Option<(u64, u64, &'static [usize])> {
    Some(match kind {
        2 => (32, 18, &[0]),        // Q4_0
        3 => (32, 20, &[0, 2]),     // Q4_1
        6 => (32, 22, &[0]),        // Q5_0
        7 => (32, 24, &[0, 2]),     // Q5_1
        8 => (32, 34, &[0]),        // Q8_0
        10 => (256, 84, &[80, 82]), // Q2_K
        11 => (256, 110, &[108]),   // Q3_K
        12 => (256, 144, &[0, 2]),  // Q4_K
        13 => (256, 176, &[0, 2]),  // Q5_K
        14 => (256, 210, &[208]),   // Q6_K
        _ => return None,
    })
}

pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, f) => {
            // Subnormal: normalize.
            let mut e = 127 - 15 + 1;
            let mut f = f;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((f & 0x3ff) << 13)
        }
        (0x1f, f) => sign | 0x7f80_0000 | (f << 13),
        (e, f) => sign | ((e + 127 - 15) << 23) | (f << 13),
    };
    f32::from_bits(bits)
}

pub fn f32_to_f16(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if frac != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (frac | 0x80_0000) >> (1 - e);
        let round = (m >> 12) & 1;
        return sign | ((m >> 13) + round) as u16;
    }
    let round = (frac >> 12) & 1;
    let h = sign as u32 | ((e as u32) << 10) | (frac >> 13);
    (h + round) as u16
}

fn bf16_to_f32(h: u16) -> f32 {
    f32::from_bits((h as u32) << 16)
}

fn f32_to_bf16(x: f32) -> u16 {
    let b = x.to_bits();
    ((b + 0x7fff + ((b >> 16) & 1)) >> 16) as u16
}

/// Scales one row's bytes (a neuron's weights) by `k`, in place.
pub fn scale_row(row: &mut [u8], kind: u32, k: f32) -> Result<(), String> {
    match kind {
        0 => row.chunks_exact_mut(4).for_each(|b| b.copy_from_slice(&(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) * k).to_le_bytes())),
        1 => row.chunks_exact_mut(2).for_each(|b| b.copy_from_slice(&f32_to_f16(f16_to_f32(u16::from_le_bytes([b[0], b[1]])) * k).to_le_bytes())),
        30 => row.chunks_exact_mut(2).for_each(|b| b.copy_from_slice(&f32_to_bf16(bf16_to_f32(u16::from_le_bytes([b[0], b[1]])) * k).to_le_bytes())),
        _ => {
            let (_, bytes, at) = block_scales(kind).ok_or("This model's weights are in a format the Lens can't edit yet.")?;
            for block in row.chunks_exact_mut(bytes as usize) {
                for &o in at {
                    let h = u16::from_le_bytes([block[o], block[o + 1]]);
                    block[o..o + 2].copy_from_slice(&f32_to_f16(f16_to_f32(h) * k).to_le_bytes());
                }
            }
        }
    }
    Ok(())
}

/// Bytes in one row of a tensor (its first dimension's values).
fn row_bytes(t: &gguf::Tensor) -> Option<u64> {
    let n = *t.dims.first()?;
    match t.kind {
        0 => Some(n * 4),
        1 | 30 => Some(n * 2),
        k => block_scales(k).filter(|(per, _, _)| n % per == 0).map(|(per, bytes, _)| n / per * bytes),
    }
}

/// Why neurons can't be edited in this model, or None when they can.
fn why_not_editable(path: &Path) -> Option<String> {
    let tensors = match gguf::tensors(path) {
        Ok(t) => t,
        Err(e) => return Some(format!("its file couldn't be read ({e})")),
    };
    let ups: Vec<&gguf::Tensor> = tensors.iter().filter(|t| t.name.ends_with(".ffn_up.weight")).collect();
    if ups.is_empty() {
        return Some(if tensors.iter().any(|t| t.name.contains("ffn_up_exps")) {
            "its feed-forward is split into experts, which the Lens doesn't edit yet".into()
        } else {
            "it has no feed-forward weights the Lens knows".into()
        });
    }
    if let Some(t) = ups.iter().find(|t| t.dims.len() != 2 || row_bytes(t).is_none()) {
        return Some(format!("its weights are in a format the Lens can't edit yet (type {})", t.kind));
    }
    // One row of ffn_up for each unit ffn_down reads; models that pack the
    // gate into ffn_up too (twice the rows) would be changed differently.
    let units = |name: &str| tensors.iter().find(|t| t.name == name).map(|t| (t.dims.get(1).copied(), t.dims.first().copied()));
    match (units("blk.0.ffn_up.weight"), units("blk.0.ffn_down.weight")) {
        (Some((rows, _)), Some((_, inputs))) if rows == inputs => None,
        _ => Some("its feed-forward weights are packed in a way the Lens doesn't edit yet".into()),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Edit {
    pub layer: u32,
    pub index: u32,
    /// 0 turns it off, 1 leaves it, 2 doubles it.
    pub scale: f32,
}

/// Writes the edits into the copy at `path` (rows of `blk.N.ffn_up`).
pub fn apply_edits(path: &Path, edits: &[Edit]) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let tensors = gguf::tensors(path).map_err(|e| format!("Couldn't read the copy: {e}"))?;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| e.to_string())?;
    for e in edits {
        let name = format!("blk.{}.ffn_up.weight", e.layer);
        let t = tensors.iter().find(|t| t.name == name).ok_or_else(|| format!("Layer {} has no feed-forward weights to edit.", e.layer))?;
        let rows = t.dims.get(1).copied().unwrap_or(0);
        if e.index as u64 >= rows {
            return Err(format!("Layer {} has {rows} neurons; there's no number {}.", e.layer, e.index));
        }
        let size = row_bytes(t).ok_or("This model's weights are in a format the Lens can't edit yet.")?;
        let at = t.start + e.index as u64 * size;
        let mut row = vec![0u8; size as usize];
        f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        f.read_exact(&mut row).map_err(|e| e.to_string())?;
        scale_row(&mut row, t.kind, e.scale)?;
        f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        f.write_all(&row).map_err(|e| e.to_string())?;
    }
    f.flush().map_err(|e| e.to_string())
}

/// A copy of the model with the edits, added to the models list (the
/// original isn't touched). Returns the copy's model id.
pub async fn edited_copy(state: &Arc<AppState>, model_id: Option<&str>, edits: Vec<Edit>) -> Result<String, String> {
    if edits.is_empty() {
        return Err("Pick at least one neuron to change.".into());
    }
    let (id, path) = model_for(state, model_id)?;
    if let Some(why) = tokio::task::spawn_blocking({
        let p = path.clone();
        move || why_not_editable(&p)
    })
    .await
    .map_err(|e| e.to_string())?
    {
        return Err(format!("This model can't be edited: {why}."));
    }
    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    let dir = state.paths.models.join("lens-edits");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if crate::storage::free_on(&dir).is_some_and(|free| free < size + (1 << 30)) {
        return Err(format!("An edited copy needs {} free where models are kept.", crate::size_label(size)));
    }
    let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) + 1;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into());
    let copy = dir.join(format!("{stem}-edited-{n}.gguf"));
    let (from, to, list) = (path.clone(), copy.clone(), edits.clone());
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        std::fs::copy(&from, &to).map_err(|e| format!("Couldn't copy the model: {e}"))?;
        apply_edits(&to, &list).inspect_err(|_| {
            let _ = std::fs::remove_file(&to);
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    crate::local::import(state, &copy).inspect_err(|_| {
        let _ = std::fs::remove_file(&copy);
    })?;
    // The original's name first: `model_spec` takes the database lock itself.
    let original = state.model_spec(&id).map(|s| s.name).unwrap_or(stem);
    let conn = state.db.lock().unwrap();
    let new = crate::local::list(&conn).into_iter().find(|m| m.local.as_ref().is_some_and(|l| Path::new(&l.path) == copy)).ok_or("The copy was made but isn't in the models list.")?;
    let what = edits.iter().map(|e| format!("L{}#{}×{}", e.layer, e.index, e.scale)).collect::<Vec<_>>().join(", ");
    let _ = db::set_model_name(&conn, &new.id, Some(&format!("{original} (edited {n})")));
    db::log_action(&conn, "model", &format!("Made an edited copy of {original}: {what}"));
    Ok(new.id)
}

// ---------- commands ----------

#[tauri::command]
pub async fn lens_words(state: AppStateRef<'_>, model_id: Option<String>, prompt: String, given: Option<String>, top: Option<u32>, most: Option<u32>) -> Result<Words, String> {
    words(state.inner(), model_id.as_deref(), &prompt, given.as_deref().unwrap_or_default(), top.unwrap_or(5), most.unwrap_or(300)).await
}

#[tauri::command]
pub async fn lens_layers(state: AppStateRef<'_>, model_id: Option<String>, text: String) -> Result<LayerMap, String> {
    if text.trim().is_empty() {
        return Err("Type a prompt first.".into());
    }
    layers(state.inner(), model_id.as_deref(), &text).await
}

#[tauri::command]
pub async fn lens_edit(state: AppStateRef<'_>, model_id: Option<String>, edits: Vec<Edit>) -> Result<String, String> {
    edited_copy(state.inner(), model_id.as_deref(), edits).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_trip() {
        for x in [0.0f32, 1.0, -2.5, 0.000_123, 65_504.0, 3.140_625, -0.007_812_5] {
            let back = f16_to_f32(f32_to_f16(x));
            assert!((back - x).abs() <= x.abs() * 1e-3 + 1e-7, "{x} came back {back}");
        }
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f32_to_f16(0.5), 0x3800);
    }

    #[test]
    fn a_row_is_scaled_through_its_block_scales() {
        // Two Q8_0 blocks: d = 1.0 then 0.5, values left as they are.
        let mut row = vec![0u8; 68];
        row[0..2].copy_from_slice(&0x3c00u16.to_le_bytes());
        row[34..36].copy_from_slice(&0x3800u16.to_le_bytes());
        row[2] = 7;
        scale_row(&mut row, 8, 2.0).unwrap();
        assert_eq!(f16_to_f32(u16::from_le_bytes([row[0], row[1]])), 2.0);
        assert_eq!(f16_to_f32(u16::from_le_bytes([row[34], row[35]])), 1.0);
        assert_eq!(row[2], 7, "the quantized values stay");
        // Q4_K: both d and dmin.
        let mut k = vec![0u8; 144];
        k[0..2].copy_from_slice(&0x3c00u16.to_le_bytes());
        k[2..4].copy_from_slice(&0x3c00u16.to_le_bytes());
        scale_row(&mut k, 12, 0.0).unwrap();
        assert_eq!((f16_to_f32(u16::from_le_bytes([k[0], k[1]])), f16_to_f32(u16::from_le_bytes([k[2], k[3]]))), (0.0, 0.0));
        assert!(scale_row(&mut [0u8; 10], 23, 2.0).is_err(), "an IQ type isn't edited");
    }

    #[test]
    fn statistics_files_are_read() {
        let s = vec![("blk.0.ffn_down.weight".to_string(), vec![1.0, 9.0, 1.0, 1.0]), ("blk.0.attn_q.weight".to_string(), vec![2.0, 2.0])];
        let back = read_imatrix(&write_imatrix(&s)).unwrap();
        assert_eq!(back, s);
        assert!(read_imatrix(&[1, 0]).is_err());
        assert_eq!(part_of("blk.12.ffn_down.weight"), Some((12, "ffn_inside")));
        assert_eq!(part_of("blk.3.attn_q.weight"), Some((3, "attention_in")));
        assert_eq!(part_of("blk.3.ffn_gate_inp.weight"), None);
        assert_eq!(part_of("output.weight"), None);
        let usual = vec![("blk.0.ffn_down.weight".to_string(), vec![1.0, 1.0, 1.0, 1.0])];
        let n = standouts(&s, &usual, 5);
        assert_eq!(n.first().map(|n| (n.layer, n.index)), Some((0, 1)), "{n:?}");
        let map = per_part(&s, 1);
        assert_eq!(map[0][PARTS.iter().position(|p| *p == "ffn_inside").unwrap()], 3.0);
    }

    #[test]
    fn pieces_come_from_either_form() {
        let new = json!({ "token": " cat", "logprob": -0.1, "top_logprobs": [{ "token": " cat", "logprob": -0.1 }, { "token": " dog", "logprob": -2.4 }] });
        let p = piece_of(&new).unwrap();
        assert_eq!(p.text, " cat");
        assert!((p.prob - 0.905).abs() < 0.01 && p.alts.len() == 2 && p.alts[1].text == " dog");
        let old = json!({ "content": " cat", "probs": [{ "tok_str": " cat", "prob": 0.8 }, { "tok_str": " dog", "prob": 0.1 }] });
        let p = piece_of(&old).unwrap();
        assert!((p.prob - 0.8).abs() < 1e-6 && p.alts.len() == 2);
    }
}

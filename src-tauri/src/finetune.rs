// SPDX-License-Identifier: AGPL-3.0-only
//! Teaching a model (LoRA fine-tuning) on the person's own data, on this
//! PC's graphics card. Training uses the QVAC Fabric build of llama.cpp
//! (`llama-finetune-lora`, Vulkan), downloaded on first use. The result is a
//! small adapter file the chat engine loads on top of the model it was
//! trained for; it appears in the model menu as "<model> + <name>".
//!
//! Training data comes from the person's chats (each question and the
//! answer it got), a file of example conversations (.jsonl, one
//! `{"messages": [...]}` per line) or documents (.txt/.md, to pick up their
//! wording). It's written unencrypted for the trainer, so it's deleted as
//! soon as training ends.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::catalog::Asset;
use crate::db;
use crate::media::proc::{self, Run};
use crate::{engine, AppState, AppStateRef};

const KEY: &str = "adapters";
const TRAINER_EXE: &str = if cfg!(windows) { "llama-finetune-lora.exe" } else { "llama-finetune-lora" };
/// Fewer examples than this don't teach anything useful.
const MIN_EXAMPLES: usize = 10;

/// QVAC Fabric b7349's build for this system: (folder key, file, size, sha256).
fn trainer_build(state: &AppState) -> Result<(&'static str, &'static str, u64, &'static str), String> {
    let backend = engine::backend_for(&state.budget())?;
    Ok(match backend {
        "vulkan-x64" | "cpu-x64" => ("vulkan-x64", "llama-b7349-bin-win-vulkan-x64.zip", 56_547_881, "eb52db3cee6937edbf727a1ecf9a2eb9204e0a13c14c7e4ba78c83815d81d0bc"),
        "linux-vulkan-x64" => ("linux-vulkan-x64", "llama-b7349-bin-ubuntu-vulkan-x64.tar.gz", 56_234_405, "fb42fb4f4b3ae623ba606f7bbbaebe620be4966e04a95a30b0fb6f9b670ce12a"),
        "linux-cpu-x64" => ("linux-cpu-x64", "llama-b7349-bin-ubuntu-x64.tar.gz", 35_714_724, "f9490b999f5f49535dea67d7fe5c839b76b011a93703c53226f8330f4b53eb23"),
        "macos-arm64" => ("macos-arm64", "llama-b7349-bin-macos-arm64.tar.gz", 31_545_987, "0c2a1058c6be5e0cf267ebea26f86af5690abfa14ffd98de00fe6838071b4fbd"),
        _ => return Err("Teaching a model isn't available on this system yet.".into()),
    })
}

fn trainer_dir(state: &AppState) -> Result<String, String> {
    Ok(format!("qvac-b7349-{}", trainer_build(state)?.0))
}

fn trainer_asset(state: &AppState) -> Result<Asset, String> {
    let (_, file, size, sha) = trainer_build(state)?;
    Ok(Asset { url: format!("https://github.com/tetherto/qvac-fabric-llm.cpp/releases/download/b7349/{file}"), size, sha256: sha.into() })
}

/// Model families the trainer supports (GGUF `general.architecture`).
const SUPPORTED: &[&str] = &["qwen3", "qwen35", "qwen35moe", "gemma3", "gemma4", "llama"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adapter {
    pub id: String,
    pub name: String,
    /// The installed model it was trained on (and only works with).
    pub base: String,
    pub base_name: String,
    pub path: String,
    /// Where the examples came from, e.g. "12 chats" or "faq.jsonl".
    pub source: String,
    pub examples: usize,
    pub epochs: u32,
    pub created: i64,
    pub final_loss: Option<f64>,
}

pub fn adapters(state: &AppState) -> Vec<Adapter> {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

fn save(state: &AppState, list: &[Adapter]) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), KEY, &list)
}

/// "adapter:<id>" in the model menu.
pub fn parse_ref(id: &str) -> Option<&str> {
    id.strip_prefix("adapter:")
}

pub fn adapter(state: &AppState, id: &str) -> Option<Adapter> {
    adapters(state).into_iter().find(|a| a.id == id)
}

// ---------- the job ----------

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub base_name: String,
    /// preparing, downloading, training, saving, done, failed, cancelled
    pub status: String,
    pub fraction: f64,
    pub epoch: u32,
    pub epochs: u32,
    pub loss: Option<f64>,
    pub eta: Option<String>,
    pub error: Option<String>,
}

static JOB: Mutex<Option<(Job, Arc<AtomicBool>)>> = Mutex::new(None);

/// A model is being taught right now (it has the graphics card).
pub fn training() -> bool {
    JOB.lock().unwrap().as_ref().is_some_and(|(j, _)| !matches!(j.status.as_str(), "done" | "failed" | "cancelled"))
}

fn update(app: &AppHandle, f: impl FnOnce(&mut Job)) {
    let snapshot = {
        let mut g = JOB.lock().unwrap();
        let Some((job, _)) = g.as_mut() else { return };
        f(job);
        job.clone()
    };
    app.emit("finetune:progress", &snapshot).ok();
}

static PROGRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"data=(\d+)/(\d+).*?loss=([\d.]+).*?ETA=(\d+:\d+:\d+)").unwrap());
static EPOCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Starting epoch (\d+)").unwrap());

/// "Starting epoch 1 (…)": the epoch starting, counted from 0.
fn parse_epoch(line: &str) -> Option<u32> {
    EPOCH.captures(line)?[1].parse().ok()
}

/// Reads a trainer progress line: (batch, batches, loss, time left).
fn parse_progress(line: &str) -> Option<(u64, u64, f64, String)> {
    let c = PROGRESS.captures(line)?;
    Some((c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?, c[4].to_string()))
}

// ---------- training data ----------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// Each question in these chats and the answer it got.
    Chats { chat_ids: Vec<String> },
    /// A .jsonl of conversations, or .txt/.md documents.
    File { path: String },
}

/// Writes the trainer's data file. Returns (examples, conversation format,
/// description).
fn write_data(state: &AppState, source: &Source, out: &Path) -> Result<(usize, bool, String), String> {
    match source {
        Source::Chats { chat_ids } => {
            let c = state.cipher()?;
            let conn = state.db.lock().unwrap();
            let mut lines = Vec::new();
            for id in chat_ids {
                let msgs = db::messages(&conn, &c, id);
                let mut question: Option<String> = None;
                let mut answer = String::new();
                let flush = |q: &mut Option<String>, a: &mut String, lines: &mut Vec<String>| {
                    if let Some(qt) = q.take() {
                        if !a.trim().is_empty() && !qt.trim().is_empty() {
                            lines.push(json!({ "messages": [{ "role": "user", "content": qt.trim() }, { "role": "assistant", "content": a.trim() }] }).to_string());
                        }
                    }
                    a.clear();
                };
                for m in msgs {
                    match m.role.as_str() {
                        "user" => {
                            flush(&mut question, &mut answer, &mut lines);
                            question = Some(m.content);
                        }
                        // The answer is the reply after any tool use: its last text.
                        "assistant" if !m.content.trim().is_empty() => answer = m.content,
                        _ => {}
                    }
                }
                flush(&mut question, &mut answer, &mut lines);
            }
            std::fs::write(out, lines.join("\n")).map_err(|e| e.to_string())?;
            Ok((lines.len(), true, format!("{} chat{}", chat_ids.len(), if chat_ids.len() == 1 { "" } else { "s" })))
        }
        Source::File { path } => {
            let p = Path::new(path);
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let text = std::fs::read_to_string(p).map_err(|e| format!("Couldn't read {name}: {e}"))?;
            if name.to_lowercase().ends_with(".jsonl") {
                let mut lines = Vec::new();
                for (i, l) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
                    let v: Value = serde_json::from_str(l).map_err(|_| format!("Line {} of {name} isn't valid JSON.", i + 1))?;
                    let ok = v["messages"].as_array().is_some_and(|m| m.iter().any(|x| x["role"] == "assistant" && x["content"].is_string()));
                    if !ok {
                        return Err(format!("Line {} of {name} needs a \"messages\" list with at least one assistant reply.", i + 1));
                    }
                    lines.push(v.to_string());
                }
                std::fs::write(out, lines.join("\n")).map_err(|e| e.to_string())?;
                Ok((lines.len(), true, name))
            } else {
                // Documents: the model reads them to pick up their wording.
                std::fs::write(out, &text).map_err(|e| e.to_string())?;
                Ok(((text.chars().count() / 2000).max(1), false, name))
            }
        }
    }
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct Base {
    id: String,
    name: String,
    /// The trainer can teach this kind of model.
    supported: bool,
}

#[derive(Serialize)]
pub struct FinetuneView {
    bases: Vec<Base>,
    adapters: Vec<Adapter>,
    job: Option<Job>,
    trainer_installed: bool,
    trainer_size: u64,
}

#[tauri::command]
pub fn finetune_view(state: AppStateRef) -> FinetuneView {
    let installed = db::installed_models(&state.db.lock().unwrap());
    let bases = installed
        .iter()
        .filter(|m| !crate::cloud::is_cloud(&m.model_id))
        .map(|m| {
            let path = crate::model_file(&state.paths, m);
            let arch = crate::gguf::read(&path).ok().and_then(|meta| meta.get("general.architecture").and_then(|v| v.str().map(str::to_string)));
            Base {
                name: state.model_spec(&m.model_id).map_or(m.model_id.clone(), |s| s.name),
                supported: arch.is_some_and(|a| SUPPORTED.contains(&a.as_str())),
                id: m.model_id.clone(),
            }
        })
        .collect();
    FinetuneView {
        bases,
        adapters: adapters(&state),
        job: JOB.lock().unwrap().as_ref().map(|(j, _)| j.clone()),
        trainer_installed: trainer_dir(&state).is_ok_and(|d| engine::find_exe(&state.paths.engines.join(d), TRAINER_EXE).is_some()),
        trainer_size: trainer_asset(&state).map_or(0, |a| a.size),
    }
}

/// How strongly the examples change the model.
fn learning_rate(strength: &str) -> &'static str {
    match strength {
        "light" => "5e-5",
        "strong" => "2e-4",
        _ => "1e-4",
    }
}

#[tauri::command]
pub async fn start_finetune(app: AppHandle, state: AppStateRef<'_>, name: String, base: String, source: Source, epochs: Option<u32>, strength: Option<String>) -> Result<(), String> {
    state.cipher()?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give it a name, like “My writing style”.".into());
    }
    if training() {
        return Err("A model is already being taught. Wait for it or stop it first.".into());
    }
    let installed = db::installed_model(&state.db.lock().unwrap(), &base).ok_or("Pick a model that's installed.")?;
    let base_name = state.model_spec(&base).map_or(base.clone(), |s| s.name);
    let id = format!("{}", uuid::Uuid::new_v4().simple());
    let dir = state.paths.data.join("adapters").join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let data = dir.join("training-data");
    let (examples, conversations, described) = match write_data(&state, &source, &data) {
        Ok(x) => x,
        Err(e) => {
            std::fs::remove_dir_all(&dir).ok();
            return Err(e);
        }
    };
    if examples < MIN_EXAMPLES {
        std::fs::remove_dir_all(&dir).ok();
        return Err(format!("That's {examples} example{}; it needs at least {MIN_EXAMPLES} to learn anything. Pick more chats or a bigger file.", if examples == 1 { "" } else { "s" }));
    }
    let epochs = epochs.unwrap_or(3).clamp(1, 10);
    let cancel = Arc::new(AtomicBool::new(false));
    *JOB.lock().unwrap() = Some((
        Job { id: id.clone(), name: name.clone(), base_name: base_name.clone(), status: "preparing".into(), fraction: 0.0, epoch: 1, epochs, loss: None, eta: None, error: None },
        cancel.clone(),
    ));
    let state = state.inner().clone();
    state.log("model", &format!("Started teaching {base_name} “{name}” from {described} ({examples} examples)"));
    tauri::async_runtime::spawn(async move {
        let result = train(&app, &state, &id, &installed, &data, conversations, epochs, strength.as_deref().unwrap_or("normal"), &cancel).await;
        // The training data was plain text: don't keep it.
        std::fs::remove_file(&data).ok();
        match result {
            Ok(loss) => {
                let path = dir.join("adapter.gguf");
                let mut list = adapters(&state);
                list.push(Adapter { id: id.clone(), name: name.clone(), base: base.clone(), base_name: base_name.clone(), path: path.display().to_string(), source: described, examples, epochs, created: db::now_ms(), final_loss: loss });
                let _ = save(&state, &list);
                std::fs::remove_dir_all(dir.join("checkpoints")).ok();
                state.log("model", &format!("Finished teaching {base_name} “{name}”; pick “{base_name} + {name}” in a chat's model menu"));
                update(&app, |j| {
                    j.status = "done".into();
                    j.fraction = 1.0;
                    j.eta = None;
                });
            }
            Err(e) if e == crate::download::CANCELLED => {
                state.log("model", &format!("Stopped teaching {base_name} “{name}”"));
                std::fs::remove_dir_all(&dir).ok();
                update(&app, |j| j.status = "cancelled".into());
            }
            Err(e) => {
                state.log("model", &format!("Teaching {base_name} “{name}” failed: {e}"));
                std::fs::remove_dir_all(&dir).ok();
                update(&app, |j| {
                    j.status = "failed".into();
                    j.error = Some(e);
                });
            }
        }
        app.emit("features:changed", json!({})).ok();
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn train(app: &AppHandle, state: &Arc<AppState>, id: &str, installed: &db::InstalledModel, data: &Path, conversations: bool, epochs: u32, strength: &str, cancel: &AtomicBool) -> Result<Option<f64>, String> {
    let dir_name = trainer_dir(state)?;
    let dir = state.paths.engines.join(&dir_name);
    if engine::find_exe(&dir, TRAINER_EXE).is_none() {
        update(app, |j| j.status = "downloading".into());
        state.log("network", "Downloading the QVAC Fabric trainer (llama.cpp with LoRA training) from github.com");
    }
    let progress = |r: u64, t: u64| update(app, |j| j.fraction = if t > 0 { r as f64 / t as f64 * 0.05 } else { 0.0 });
    let exe = engine::ensure_unpacked(&state.paths, &dir_name, &trainer_asset(state)?, TRAINER_EXE, cancel, progress).await?;
    // The chat model gives the graphics card back for the training run.
    state.engine.lock().await.stop().await;

    let out_dir = data.parent().unwrap_or(Path::new(".")).to_path_buf();
    let model = crate::model_file(&state.paths, installed);
    let mut args: Vec<std::ffi::OsString> = vec!["-m".into(), model.into_os_string(), "-f".into(), data.as_os_str().to_owned()];
    if conversations {
        args.push("--assistant-loss-only".into());
    }
    for a in [
        "--lora-rank", "16", "--lora-alpha", "32", "--learning-rate", learning_rate(strength), "--num-epochs", &epochs.to_string(),
        "-c", "512", "-b", "512", "-ub", "512", "-ngl", "999", "-fa", "off", "--checkpoint-save-steps", "50", "--auto-resume",
    ] {
        args.push(a.into());
    }
    args.push("--output-adapter".into());
    args.push(out_dir.join("adapter.gguf").into_os_string());
    args.push("--checkpoint-save-dir".into());
    args.push(out_dir.join("checkpoints").into_os_string());

    update(app, |j| j.status = "training".into());
    let mut epoch = 1u32;
    // Training batches per epoch; the smaller counts are the held-out check.
    let mut train_batches = 0u64;
    let mut loss = None;
    let log = state.paths.logs.join(format!("finetune-{id}.log"));
    proc::run(Run { exe: &exe, args, cwd: &out_dir, log: &log, low_priority: false, job: state.job() }, cancel, |line| {
        if let Some(e) = parse_epoch(line) {
            epoch = e + 1;
        }
        if let Some((batch, batches, l, eta)) = parse_progress(line) {
            train_batches = train_batches.max(batches);
            if batches < train_batches {
                return;
            }
            loss = Some(l);
            let done = (epoch - 1) as f64 + batch as f64 / batches.max(1) as f64;
            update(app, |j| {
                j.epoch = epoch.min(epochs);
                j.loss = Some(l);
                j.fraction = 0.05 + 0.95 * (done / epochs as f64).min(1.0);
                j.eta = Some(eta);
            });
        }
    })
    .await?;
    if !out_dir.join("adapter.gguf").exists() {
        return Err("The trainer finished without saving the adapter. Its log is in the app's logs folder.".into());
    }
    Ok(loss)
}

#[tauri::command]
pub fn cancel_finetune() {
    if let Some((_, cancel)) = JOB.lock().unwrap().as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn remove_adapter(state: AppStateRef, id: String) -> Result<(), String> {
    let mut list = adapters(&state);
    let Some(a) = list.iter().position(|a| a.id == id).map(|i| list.remove(i)) else { return Ok(()) };
    save(&state, &list)?;
    let dir = state.paths.data.join("adapters").join(&a.id);
    if dir.starts_with(state.paths.data.join("adapters")) {
        std::fs::remove_dir_all(&dir).ok();
    }
    state.log("model", &format!("Removed the taught model “{} + {}”", a.base_name, a.name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_trainer_progress() {
        let line = "train: [██▎     ] data=0000011/0000038 loss=4.71471±0.49001 acc=53.82±3.01% t=00:05:34 ETA=00:13:39";
        assert_eq!(parse_progress(line), Some((11, 38, 4.71471, "00:13:39".to_string())));
        assert_eq!(parse_progress("llama_model_loader: loaded meta data"), None);
        assert_eq!(parse_epoch("Starting epoch 1 (step 37, lr=1.0000e-04)"), Some(1));
    }

    #[test]
    fn adapter_refs() {
        assert_eq!(parse_ref("adapter:abc"), Some("abc"));
        assert_eq!(parse_ref("qwen3-1.7b"), None);
    }

    #[test]
    fn sources_parse_from_the_window() {
        let s: Source = serde_json::from_value(json!({ "kind": "chats", "chat_ids": ["a", "b"] })).unwrap();
        assert!(matches!(s, Source::Chats { chat_ids } if chat_ids.len() == 2));
        let f: Source = serde_json::from_value(json!({ "kind": "file", "path": "C:/x.jsonl" })).unwrap();
        assert!(matches!(f, Source::File { .. }));
    }
}

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
/// At startup: what an interrupted teaching run left behind, above all its
/// training data (plain text from the person's chats). A finished model
/// keeps only its adapter; logs go after 30 days.
pub fn clean_leftovers(paths: &crate::paths::Paths) {
    if let Ok(entries) = std::fs::read_dir(paths.data.join("adapters")) {
        for e in entries.flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            if dir.join("adapter.gguf").exists() {
                std::fs::remove_file(dir.join("training-data")).ok();
                std::fs::remove_file(dir.join("train-model.gguf")).ok();
            } else {
                std::fs::remove_dir_all(&dir).ok();
            }
        }
    }
    let month = std::time::Duration::from_secs(30 * 86_400);
    if let Ok(entries) = std::fs::read_dir(&paths.logs) {
        for e in entries.flatten() {
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > month);
            if old && e.file_name().to_string_lossy().starts_with("finetune-") {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }
}

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

/// The model file's family (GGUF `general.architecture`).
pub(crate) fn arch_of(state: &AppState, m: &db::InstalledModel) -> Option<String> {
    let path = crate::model_file(&state.paths, m);
    crate::gguf::read(&path).ok().and_then(|meta| meta.get("general.architecture").and_then(|v| v.str().map(str::to_string)))
}

pub(crate) fn supported(arch: &str) -> bool {
    SUPPORTED.contains(&arch)
}

/// Families whose replies the trainer can find in a conversation (it looks
/// for ChatML's and Gemma 3's turn markers). Llama 3 and Gemma 4 lay out
/// turns their own way, so they learn from documents only.
pub(crate) fn conversations_ok(arch: &str) -> bool {
    matches!(arch, "qwen3" | "qwen35" | "qwen35moe" | "gemma3")
}

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
    /// Taught by practice: how it did on the test before and after.
    #[serde(default)]
    pub scores: Option<Scores>,
    /// The model that answered the practice tasks it missed, if any.
    #[serde(default)]
    pub teacher: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scores {
    pub before: u32,
    pub after: u32,
    pub out_of: u32,
    /// Skill by skill (practice kept before this have none).
    #[serde(default)]
    pub skills: Vec<SkillScore>,
    /// The second test, of new tasks, that confirmed it.
    #[serde(default)]
    pub second: Option<Second>,
}

/// Practice's second test, of new tasks: before and after, in all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Second {
    pub before: u32,
    pub after: u32,
    pub out_of: u32,
}

/// How one skill's test tasks went, before and after.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillScore {
    pub skill: crate::practice::tasks::Skill,
    pub before: u32,
    pub after: u32,
    pub out_of: u32,
    /// The adapter learnt from this skill's practice.
    pub learnt: bool,
}

pub fn adapters(state: &AppState) -> Vec<Adapter> {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

pub(crate) fn save(state: &AppState, list: &[Adapter]) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), KEY, &list)
}

/// A model was removed: the models taught on top of it (which only work
/// with it) go too. Returns how many.
pub fn forget_adapters_of(state: &AppState, base: &str) -> usize {
    let mut list = adapters(state);
    let root = state.paths.data.join("adapters");
    let gone: Vec<Adapter> = list.iter().filter(|a| a.base == base).cloned().collect();
    for a in &gone {
        let dir = root.join(&a.id);
        if dir.starts_with(&root) && dir != root {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    if !gone.is_empty() {
        list.retain(|a| a.base != base);
        let _ = save(state, &list);
    }
    gone.len()
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
    /// preparing, downloading, training, saving, done, failed, cancelled;
    /// practice adds testing, practising, teacher, checking and not_better.
    pub status: String,
    pub fraction: f64,
    pub epoch: u32,
    pub epochs: u32,
    pub loss: Option<f64>,
    pub eta: Option<String>,
    pub error: Option<String>,
    /// Practice: tasks done of this stage's total, examples kept so far
    /// (the teacher's, in its turn), and the test's scores before and after.
    pub done: u32,
    pub total: u32,
    pub kept: u32,
    pub before: Option<u32>,
    pub after: Option<u32>,
    /// Practice with a teacher: its name.
    pub teacher: Option<String>,
    /// Practice learns each skill on its own first: the skills being
    /// learnt or tested now, and which try this is of how many.
    pub part: Vec<crate::practice::tasks::Skill>,
    pub part_no: u32,
    pub parts: u32,
    /// Practice, at the end: the test skill by skill, for what was kept (or
    /// the best try, when nothing was).
    pub skills: Vec<SkillScore>,
    /// Practice: the second test, of new tasks, of the one it would keep.
    pub second: Option<Second>,
    /// Training doesn't fit the card's free memory, or has slowed down as
    /// if it didn't: closing other apps that use the card helps.
    pub low_memory: bool,
}

impl Job {
    pub(crate) fn new(id: &str, name: &str, base_name: &str, epochs: u32) -> Job {
        Job {
            id: id.into(),
            name: name.into(),
            base_name: base_name.into(),
            status: "preparing".into(),
            fraction: 0.0,
            epoch: 1,
            epochs,
            loss: None,
            eta: None,
            error: None,
            done: 0,
            total: 0,
            kept: 0,
            before: None,
            after: None,
            teacher: None,
            part: Vec::new(),
            part_no: 0,
            parts: 0,
            skills: Vec::new(),
            second: None,
            low_memory: false,
        }
    }
}

static JOB: Mutex<Option<(Job, Arc<AtomicBool>)>> = Mutex::new(None);

/// A model is being taught right now (it has the graphics card).
pub fn training() -> bool {
    JOB.lock().unwrap().as_ref().is_some_and(|(j, _)| !matches!(j.status.as_str(), "done" | "failed" | "cancelled" | "not_better"))
}

/// Starts a teaching job, unless one is running. Its stop flag.
pub(crate) fn begin(job: Job) -> Result<Arc<AtomicBool>, String> {
    let mut g = JOB.lock().unwrap();
    if g.as_ref().is_some_and(|(j, _)| !matches!(j.status.as_str(), "done" | "failed" | "cancelled" | "not_better")) {
        return Err("A model is already being taught. Wait for it or stop it first.".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *g = Some((job, cancel.clone()));
    Ok(cancel)
}

pub(crate) fn update(app: &AppHandle, f: impl FnOnce(&mut Job)) {
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
    /// It can learn from conversations (chats, .jsonl), not just documents.
    conversations: bool,
    /// It can practise (its chat format is one the trainer reads).
    practice: bool,
    /// It can use tools, so it practises that too.
    tools: bool,
}

#[derive(Serialize)]
pub struct FinetuneView {
    bases: Vec<Base>,
    adapters: Vec<Adapter>,
    job: Option<Job>,
    trainer_installed: bool,
    trainer_size: u64,
}

#[tauri::command(async)]
pub fn finetune_view(state: AppStateRef) -> FinetuneView {
    let installed = db::installed_models(&state.db.lock().unwrap());
    let bases = installed
        .iter()
        .filter(|m| !crate::cloud::is_cloud(&m.model_id))
        .map(|m| {
            let arch = arch_of(&state, m);
            let spec = state.model_spec(&m.model_id);
            Base {
                name: spec.as_ref().map_or(m.model_id.clone(), |s| s.name.clone()),
                supported: arch.as_deref().is_some_and(supported),
                conversations: arch.as_deref().is_some_and(conversations_ok),
                practice: arch.as_deref().is_some_and(crate::practice::can_practise),
                tools: spec.is_some_and(|s| s.tools),
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
fn learning_rate(strength: &str, own_words: bool) -> &'static str {
    // A model's own replies are already likely to it, so learning from them
    // needs a much gentler rate: at 1e-4 a 4B's loss on them climbed from
    // 0.12 to 1.8 over two rounds and it lost every test task.
    match (strength, own_words) {
        ("light", true) => "1e-5",
        ("strong", true) => "5e-5",
        (_, true) => "2e-5",
        ("light", _) => "5e-5",
        ("strong", _) => "2e-4",
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
    fits_card(&state, &installed)?;
    let base_name = state.model_spec(&base).map_or(base.clone(), |s| s.name);
    let documents = matches!(&source, Source::File { path } if !path.to_lowercase().ends_with(".jsonl"));
    if !documents && !arch_of(&state, &installed).is_some_and(|a| conversations_ok(&a)) {
        return Err("This model can only be taught from documents (.txt or .md): the trainer can't find its replies in its chat format (Llama and Gemma 4).".into());
    }
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
    let cancel = match begin(Job::new(&id, &name, &base_name, epochs)) {
        Ok(c) => c,
        Err(e) => {
            std::fs::remove_dir_all(&dir).ok();
            return Err(e);
        }
    };
    let state = state.inner().clone();
    state.log("model", &format!("Started teaching {base_name} “{name}” from {described} ({examples} examples)"));
    tauri::async_runtime::spawn(async move {
        let setup = Setup { conversations, epochs, strength: strength.as_deref().unwrap_or("normal"), own_words: false, ctx: 512, template: None, span: (0.0, 1.0) };
        let result = train(&app, &state, &id, &installed, &data, &setup, &cancel).await;
        // The training data was plain text: don't keep it.
        std::fs::remove_file(&data).ok();
        match result {
            Ok(loss) => {
                let path = dir.join("adapter.gguf");
                let mut list = adapters(&state);
                list.push(Adapter {
                    id: id.clone(),
                    name: name.clone(),
                    base: base.clone(),
                    base_name: base_name.clone(),
                    path: path.display().to_string(),
                    source: described,
                    examples,
                    epochs,
                    created: db::now_ms(),
                    final_loss: loss,
                    scores: None,
                    teacher: None,
                });
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

/// The trainer, downloaded the first time. `span`: the share of the job's
/// progress bar the download fills.
pub(crate) async fn ensure_trainer(app: &AppHandle, state: &Arc<AppState>, cancel: &AtomicBool, span: f64) -> Result<std::path::PathBuf, String> {
    let dir_name = trainer_dir(state)?;
    let dir = state.paths.engines.join(&dir_name);
    if engine::find_exe(&dir, TRAINER_EXE).is_none() {
        update(app, |j| j.status = "downloading".into());
        state.log("network", "Downloading the QVAC Fabric trainer (llama.cpp with LoRA training) from github.com");
    }
    let progress = |r: u64, t: u64| update(app, |j| j.fraction = if t > 0 { r as f64 / t as f64 * span } else { 0.0 });
    engine::ensure_unpacked(&state.paths, &dir_name, &trainer_asset(state)?, TRAINER_EXE, cancel, progress).await
}

/// How a training run goes.
pub(crate) struct Setup<'a> {
    /// Conversations (`{"messages": …}` lines, trained on the replies only)
    /// rather than documents.
    pub conversations: bool,
    pub epochs: u32,
    pub strength: &'a str,
    /// The examples are the model's own replies (practice): a gentler rate
    /// that warms up and then tapers off.
    pub own_words: bool,
    /// Tokens per example: longer conversations are left out by the trainer.
    pub ctx: u32,
    /// A Jinja chat template to render the conversations with, instead of
    /// the model's own.
    pub template: Option<&'a Path>,
    /// The share of the job's progress bar the run fills (from, to).
    pub span: (f64, f64),
}

/// Quantizations the trainer runs wholly on the graphics card. With the
/// K-quants most models come in, part of each step falls back to the
/// processor: measured on a 4B, 50 s a 512-token example instead of 10
/// (Q4_0) or 6 (Q8_0).
fn trains_on_card(quant: &str) -> bool {
    matches!(quant.to_ascii_uppercase().as_str(), "Q4_0" | "Q8_0" | "F16" | "BF16" | "F32")
}

/// Bits a weight takes in a model file of this quantization, embeddings
/// and all (Qwen3 4B 2507: Q4_K_M 4.97, Q4_0 4.71, Q8_0 8.51), to tell how
/// many weights a file holds and how big a copy in another format comes out.
/// Rounded so the weights are overcounted, never under.
fn bits(quant: &str) -> f64 {
    match quant.to_ascii_uppercase().as_str() {
        "F32" => 32.0,
        "F16" | "BF16" => 16.0,
        "Q8_0" => 8.5,
        "Q6_K" => 6.5,
        "Q4_0" => 4.7,
        q if q.starts_with("Q5") => 5.5,
        q if q.starts_with("Q3") || q.starts_with("IQ3") => 3.8,
        q if q.starts_with("Q2") || q.starts_with("IQ2") || q.starts_with("IQ1") => 2.8,
        // Q4_K_M, as the catalog's models come, and the like.
        _ => 4.9,
    }
}

/// Card memory a training step takes per weight besides the weights
/// themselves, at the 512-token window teaching and practice use: the LoRA
/// and its optimizer, the activations kept for the backward pass, the
/// vocabulary's logits. Measured with the trainer's own counters on Qwen3
/// 4B 2507 (2026-10-19): its Q4_0 copy (2.37 GB) held 7.95 GB of the card
/// throughout; its Q8_0 copy (4.28 GB) held 9.8 GB, and at step 8 asked
/// for 1.1 GB more (the step where app runs crashed or crawled). Both also
/// keep about 0.7 GB in the computer's memory, which isn't the card's.
fn per_weight(target: &str) -> f64 {
    if target.eq_ignore_ascii_case("Q4_0") {
        1.5
    } else {
        1.8
    }
}

/// Card memory left free beyond training's measured peak, since the free
/// memory is read once, at the start, and the desktop grows with another
/// window or a video. Going over the card's memory isn't just slow: on his
/// RTX 5070 the 4B's Q8_0 step 8 took the card to 12,087 of 12,227 MB, the
/// NVIDIA driver reset (nvlddmkm event 153), the trainer crashed
/// (0xC0000409, as in two practice runs that "crashed at step 8") and the
/// NVIDIA overlay was left as a black layer over his main monitor.
const HEADROOM: u64 = 1 << 30;
/// Card memory the desktop takes at the least, for the check at the start.
const DESKTOP: u64 = 512 << 20;
/// How long training waits for other apps to give card memory back.
const ROOM_WAIT: std::time::Duration = std::time::Duration::from_secs(180);

pub const NO_ROOM: &str = "There isn't enough free memory on the graphics card to teach this model safely. Close other apps that use it, such as games, browsers and video players, and try again.";
pub const TOO_BIG: &str = "This model is too big to teach on this graphics card. Pick a smaller model.";

/// Card memory training takes, for a model whose installed file is `size`
/// bytes of `quant`, trained as `target`.
fn training_need(size: u64, quant: &str, target: &str) -> u64 {
    let weights = size as f64 * 8.0 / bits(quant);
    (weights * (bits(target) / 8.0 + per_weight(target))) as u64
}

/// What a model is trained as: the installed file (`copy` None) or a copy in
/// another format, the card memory that takes, and whether that fits.
#[derive(Debug, PartialEq)]
struct Plan {
    copy: Option<&'static str>,
    need: u64,
    fits: bool,
}

/// The most faithful format whose training fits `free` card memory with
/// room to spare. A file the card trains as it is stays as it is when it
/// fits, and is only ever copied smaller; other formats are copied to Q8_0
/// or Q4_0. When nothing fits, the smallest, flagged. Without a graphics
/// card (`free` None) there's nothing to run out of: the smallest.
fn plan(size: u64, quant: &str, free: Option<u64>) -> Plan {
    let copies = ["Q8_0", "Q4_0"].into_iter();
    let options: Vec<Option<&'static str>> = if trains_on_card(quant) {
        std::iter::once(None).chain(copies.filter(|t| bits(t) < bits(quant)).map(Some)).collect()
    } else {
        copies.map(Some).collect()
    };
    let need = |o: Option<&str>| training_need(size, quant, o.unwrap_or(quant));
    let fitting = free.and_then(|free| options.iter().copied().find(|o| need(*o) + HEADROOM <= free));
    let copy = fitting.unwrap_or(options[options.len() - 1]);
    Plan { copy, need: need(copy), fits: fitting.is_some() || free.is_none() }
}

/// Before teaching or practice starts (so practice isn't wasted): whether
/// the model can train on this PC's graphics card at all, in its smallest
/// format, with the headroom and the least the desktop takes.
pub(crate) fn fits_card(state: &AppState, installed: &db::InstalledModel) -> Result<(), String> {
    let total = state.hardware.read().unwrap().best_gpu().map_or(0, |g| g.vram_bytes);
    if too_big(model_size(state, installed), &installed.quant, total) {
        return Err(TOO_BIG.into());
    }
    Ok(())
}

fn too_big(size: u64, quant: &str, card: u64) -> bool {
    card > 0 && plan(size, quant, Some(0)).need + HEADROOM + DESKTOP > card
}

/// The model's size on disk: all its parts when it's split across files
/// (`model_file` is the first).
fn model_size(state: &AppState, installed: &db::InstalledModel) -> u64 {
    let first = std::fs::metadata(crate::model_file(&state.paths, installed)).map_or(0, |m| m.len());
    installed.size.max(first)
}

/// What the graphics card has free now (the chat model has just been
/// stopped): its size less what Windows, the desktop and other apps hold.
/// None without a graphics card. Windows' count lags a moment behind a
/// program that has just quit, so it's read until it stops falling.
async fn card_free(state: &AppState) -> Option<u64> {
    let total = state.hardware.read().unwrap().best_gpu().map_or(0, |g| g.vram_bytes);
    if total == 0 {
        return None;
    }
    let card = crate::hardware::dedicated().and_then(|c| c.luid);
    let mut used = crate::perf::card_memory_used(card);
    for _ in 0..6 {
        let Some(before) = used else { break };
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        used = crate::perf::card_memory_used(card);
        if used.is_none_or(|now| before.saturating_sub(now) < 64 << 20) {
            break;
        }
    }
    Some(total.saturating_sub(used.unwrap_or(3 << 29)))
}

/// The model file to train on: the installed one, or a copy in a format the
/// card trains fast, made next to the training data and deleted after.
///
/// Q8_0 where it fits: copied from the installed file it keeps the same
/// weights, so what's learnt carries over. A Q4_0 copy rounds them again
/// onto another grid: on a 4B its loss on the model's own replies started
/// at 0.94 instead of 0.67, so part of training went into undoing that, and
/// the adapter did worse on the real model. Q4_0 is kept for when the Q8_0
/// copy's training wouldn't fit the card's free memory (the before/after
/// test still decides). Training that outgrows the card first slows down
/// (Windows moves part of it to the computer's memory: 7 s a step became
/// 37 s), then can reset the graphics driver (`HEADROOM`). So when nothing
/// fits with room to spare, it waits a few minutes for other apps to give
/// memory back, and then doesn't start.
async fn training_copy(app: &AppHandle, state: &Arc<AppState>, installed: &db::InstalledModel, trainer: &Path, out_dir: &Path, cancel: &AtomicBool) -> Result<std::path::PathBuf, String> {
    let model = crate::model_file(&state.paths, installed);
    let size = model_size(state, installed);
    let mut free = card_free(state).await;
    let mut chosen = plan(size, &installed.quant, free);
    let waiting = std::time::Instant::now();
    if !chosen.fits {
        let format = chosen.copy.unwrap_or(&installed.quant);
        let (need, free) = (crate::size_label(chosen.need), crate::size_label(free.unwrap_or(0)));
        state.log("model", &format!("Training as {format} needs about {need} of the graphics card's memory and 1 GB to spare, but only {free} is free; waiting up to 3 minutes for other apps to give some back"));
        update(app, |j| j.low_memory = true);
    }
    while !chosen.fits {
        if waiting.elapsed() > ROOM_WAIT {
            state.log("model", "The graphics card's memory didn't free up, so training didn't start");
            return Err(NO_ROOM.into());
        }
        for _ in 0..5 {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::download::CANCELLED.into());
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        free = card_free(state).await;
        chosen = plan(size, &installed.quant, free);
        if chosen.fits {
            update(app, |j| j.low_memory = false);
        }
    }
    if let Some(free) = free {
        let format = chosen.copy.unwrap_or(&installed.quant);
        state.log("model", &format!("Training as {format}: it needs about {} of the graphics card's memory, and {} is free", crate::size_label(chosen.need), crate::size_label(free)));
    }
    let Some(target) = chosen.copy else { return Ok(model) };
    let need = (size as f64 * bits(target) / bits(&installed.quant)) as u64 + 512 * 1024 * 1024;
    if crate::storage::free_on(out_dir).is_some_and(|free| free < need) {
        return Err("There isn't enough free disk space for the model's training copy. Free a few gigabytes on the drive the app keeps its data on, and try again.".into());
    }
    let quantize = engine::find_exe(trainer.parent().unwrap_or(Path::new(".")), if cfg!(windows) { "llama-quantize.exe" } else { "llama-quantize" })
        .ok_or("The trainer's model converter is missing. Remove the trainer folder in the app's engines folder to download it again.")?;
    let copy = out_dir.join("train-model.gguf");
    let threads = state.limits().threads.max(1).to_string();
    let args: Vec<std::ffi::OsString> = vec!["--allow-requantize".into(), model.into_os_string(), copy.clone().into_os_string(), target.into(), threads.into()];
    let log = state.paths.logs.join("finetune-copy.log");
    proc::run(Run { exe: &quantize, args, cwd: out_dir, log: &log, low_priority: true, job: state.job() }, cancel, |_| {}).await?;
    if !copy.exists() {
        return Err("Making the model's training copy failed. Its log is in the app's logs folder.".into());
    }
    state.log("model", &format!("Made a {target} copy of the model to train on (the card trains it several times faster)"));
    Ok(copy)
}

pub(crate) async fn train(app: &AppHandle, state: &Arc<AppState>, id: &str, installed: &db::InstalledModel, data: &Path, setup: &Setup<'_>, cancel: &AtomicBool) -> Result<Option<f64>, String> {
    let (from, to) = setup.span;
    let out_dir = data.parent().unwrap_or(Path::new("."));
    let ready = prepare(app, state, installed, out_dir, from + (to - from) * 0.05, cancel).await?;
    ready.train(app, state, id, data, setup, cancel).await
}

/// The trainer and the model file it trains, ready for one training run or
/// several (practice trains one adapter per skill). A training copy is
/// deleted when this is dropped.
pub(crate) struct Ready {
    exe: std::path::PathBuf,
    model: std::path::PathBuf,
    copy: bool,
}

impl Drop for Ready {
    fn drop(&mut self) {
        if self.copy {
            std::fs::remove_file(&self.model).ok();
        }
    }
}

impl Ready {
    /// Trains an adapter on `data`, saved next to it as adapter.gguf.
    pub(crate) async fn train(&self, app: &AppHandle, state: &Arc<AppState>, id: &str, data: &Path, setup: &Setup<'_>, cancel: &AtomicBool) -> Result<Option<f64>, String> {
        // The chat model gives the graphics card back for the training run.
        state.engine.lock().await.stop().await;
        run_trainer(app, state, id, &self.exe, &self.model, data, setup, cancel).await
    }
}

/// Downloads the trainer the first time (filling the progress bar up to
/// `span`) and makes the model's training copy in `out_dir`.
pub(crate) async fn prepare(app: &AppHandle, state: &Arc<AppState>, installed: &db::InstalledModel, out_dir: &Path, span: f64, cancel: &AtomicBool) -> Result<Ready, String> {
    let exe = ensure_trainer(app, state, cancel, span).await?;
    // The chat model gives the graphics card back, so its free memory can be measured.
    state.engine.lock().await.stop().await;
    // One plan and one copy, for every adapter trained from it.
    let model = training_copy(app, state, installed, &exe, out_dir, cancel).await?;
    let copy = model.starts_with(out_dir);
    Ok(Ready { exe, model, copy })
}

pub const TRAINER_STALLED: &str = "The trainer stopped making progress, most likely because the graphics card ran out of memory. Close other apps that use it, such as games, browsers and video players, and try again.";
pub const TRAINER_QUIT: &str = "The trainer quit partway through, most likely because the graphics card ran out of memory. Close other apps that use it, such as games, browsers and video players, and try again.";

/// How long the first step may take, loading the model included.
const FIRST_STEP: f64 = 15.0 * 60.0;
/// A run has stalled when a step takes this many times the usual one, and
/// at least `STALL_FLOOR` seconds.
const STALL_TIMES: f64 = 15.0;
const STALL_FLOOR: f64 = 4.0 * 60.0;
/// Three steps in a row this many times the usual one: short of memory.
const SLOW_TIMES: f64 = 4.0;

/// The trainer's pace, to stop a run that has stalled. When training
/// outgrows the card, Windows moves part of its memory to the computer's,
/// and steps go from seconds to minutes and then stop: one run went from
/// 7 s a step to 37 s at step 8, then sat on step 13 for 28 minutes. Times
/// are seconds since the trainer started.
#[derive(Default)]
struct Pace {
    /// How long each training step took (the first includes loading).
    took: Vec<f64>,
    /// When the last step ended, a held-out check's step too.
    last: f64,
    /// When the pace was last looked at.
    seen: f64,
    /// Slow steps in a row.
    slow: u32,
}

impl Pace {
    /// The usual step: the middle one of steps 2-6, or the first while
    /// that's all there is.
    fn usual(&self) -> Option<f64> {
        let mut early: Vec<f64> = self.took.iter().skip(1).take(5).copied().collect();
        if early.is_empty() {
            return self.took.first().copied();
        }
        early.sort_by(f64::total_cmp);
        Some(early[early.len() / 2])
    }

    /// Called first whenever the pace is looked at. A long gap means this
    /// PC slept, with the trainer paused too, so the wait starts again.
    fn awake(&mut self, now: f64) {
        if now - self.seen > 30.0 {
            self.last += now - self.seen;
        }
        self.seen = now;
    }

    /// A step ended (`training`, or the held-out check's). When steps have
    /// just turned slow: (the usual step, this one).
    fn step(&mut self, now: f64, training: bool) -> Option<(f64, f64)> {
        let took = now - self.last;
        self.last = now;
        if !training {
            return None;
        }
        self.took.push(took);
        let usual = self.usual().filter(|_| self.took.len() > 6)?;
        self.slow = if took > usual * SLOW_TIMES { self.slow + 1 } else { 0 };
        (self.slow == 3).then_some((usual, took))
    }

    /// How long it has waited for the next step.
    fn waited(&self, now: f64) -> f64 {
        now - self.last
    }

    /// No step for far longer than the usual one, or the first step not
    /// done in `FIRST_STEP`.
    fn stalled(&self, now: f64) -> bool {
        let limit = self.usual().map_or(FIRST_STEP, |u| (u * STALL_TIMES).max(STALL_FLOOR));
        self.waited(now) > limit
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_trainer(app: &AppHandle, state: &Arc<AppState>, id: &str, exe: &Path, model: &Path, data: &Path, setup: &Setup<'_>, cancel: &AtomicBool) -> Result<Option<f64>, String> {
    let (from, to) = setup.span;
    let out_dir = data.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut args: Vec<std::ffi::OsString> = vec!["-m".into(), model.as_os_str().to_owned(), "-f".into(), data.as_os_str().to_owned()];
    if setup.conversations {
        args.push("--assistant-loss-only".into());
    }
    if let Some(t) = setup.template {
        args.push("--chat-template".into());
        args.push(t.as_os_str().to_owned());
    }
    let (epochs, ctx) = (setup.epochs, setup.ctx.to_string());
    for a in [
        "--lora-rank", "16", "--lora-alpha", "32", "--learning-rate", learning_rate(setup.strength, setup.own_words), "--num-epochs", &epochs.to_string(),
        "-c", &ctx, "-b", &ctx, "-ub", &ctx, "-ngl", "999", "-fa", "off", "--checkpoint-save-steps", "50", "--auto-resume",
    ] {
        args.push(a.into());
    }
    if setup.own_words {
        for a in ["--lr-scheduler", "cosine", "--warmup-ratio", "0.1"] {
            args.push(a.into());
        }
    }
    args.push("--output-adapter".into());
    args.push(out_dir.join("adapter.gguf").into_os_string());
    args.push("--checkpoint-save-dir".into());
    args.push(out_dir.join("checkpoints").into_os_string());

    update(app, |j| j.status = "training".into());
    let start = from + (to - from) * 0.05;
    let mut epoch = 1u32;
    // Training batches per epoch; the smaller counts are the held-out check.
    let mut train_batches = 0u64;
    let mut loss = None;
    // Examples longer than the trainer's window, which it leaves out.
    let mut too_long = 0usize;
    let log = state.paths.logs.join(format!("finetune-{id}.log"));
    let began = std::time::Instant::now();
    let mut pace = Pace::default();
    let watched = proc::run_watched(Run { exe, args, cwd: &out_dir, log: &log, low_priority: false, job: state.job() }, cancel, |line| {
        let now = began.elapsed().as_secs_f64();
        pace.awake(now);
        let line = line.unwrap_or_default();
        if line.contains("Skipping conversation") && line.contains("too long") {
            too_long += 1;
        }
        if let Some(e) = parse_epoch(line) {
            epoch = e + 1;
        }
        if let Some((batch, batches, l, eta)) = parse_progress(line) {
            train_batches = train_batches.max(batches);
            let training = batches == train_batches;
            if let Some(slowed) = pace.step(now, training) {
                state.log("model", &format!("Training slowed from about {:.0} s a step to {:.0} s: the graphics card looks short of memory", slowed.0, slowed.1));
                update(app, |j| j.low_memory = true);
            }
            if training {
                loss = Some(l);
                let done = (epoch - 1) as f64 + batch as f64 / batches.max(1) as f64;
                update(app, |j| {
                    j.epoch = epoch.min(epochs);
                    j.loss = Some(l);
                    j.fraction = start + (to - start) * (done / epochs as f64).min(1.0);
                    j.eta = Some(eta);
                });
            }
        }
        pace.stalled(now).then(|| TRAINER_STALLED.to_string())
    })
    .await;
    if let Err(e) = watched {
        let steps = pace.took.len();
        let card = crate::hardware::dedicated().and_then(|c| c.luid);
        let used = crate::perf::card_memory_used(card).map_or(String::new(), |u| format!("; the graphics card had {} in use", crate::size_label(u)));
        if e == TRAINER_STALLED {
            state.log("model", &format!("The trainer made no progress for {:.0} minutes after {steps} steps (they took about {:.0} s each){used}, so it was stopped", pace.waited(began.elapsed().as_secs_f64()) / 60.0, pace.usual().unwrap_or(0.0)));
            return Err(e);
        }
        // Quitting partway, after steps that worked: the card's memory ran
        // out (a 4B's Q8_0 run crashed at step 8 with 0xC0000409).
        if steps > 0 && e != crate::download::CANCELLED {
            state.log("model", &format!("The trainer quit after {steps} steps{used}: {e}"));
            return Err(TRAINER_QUIT.into());
        }
        return Err(e);
    }
    if too_long > 0 {
        state.log("model", &format!("{too_long} example{} longer than {} tokens, which the trainer leaves out", if too_long == 1 { " was" } else { "s were" }, setup.ctx));
    }
    if !out_dir.join("adapter.gguf").exists() {
        return Err("The trainer finished without saving the adapter. Its log is in the app's logs folder.".into());
    }
    Ok(loss)
}

#[tauri::command(async)]
pub fn cancel_finetune() {
    if let Some((_, cancel)) = JOB.lock().unwrap().as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command(async)]
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

    /// After a crash mid-teaching: its plain-text training data is gone at
    /// the next start; a finished model keeps its adapter.
    #[test]
    fn an_interrupted_teaching_run_leaves_nothing_behind() {
        let paths = crate::paths::Paths::new(std::env::temp_dir().join(format!("sulcusai-ft-{}", uuid::Uuid::new_v4()))).unwrap();
        let (cut, done) = (paths.data.join("adapters").join("cut"), paths.data.join("adapters").join("done"));
        std::fs::create_dir_all(&cut).unwrap();
        std::fs::create_dir_all(&done).unwrap();
        std::fs::write(cut.join("training-data"), "my private chats").unwrap();
        std::fs::write(done.join("adapter.gguf"), "weights").unwrap();
        std::fs::write(done.join("training-data"), "my private chats").unwrap();
        clean_leftovers(&paths);
        assert!(!cut.exists(), "an unfinished run goes entirely");
        assert!(done.join("adapter.gguf").exists() && !done.join("training-data").exists());
        std::fs::remove_dir_all(&paths.data).ok();
    }

    #[test]
    fn reads_trainer_progress() {
        let line = "train: [██▎     ] data=0000011/0000038 loss=4.71471±0.49001 acc=53.82±3.01% t=00:05:34 ETA=00:13:39";
        assert_eq!(parse_progress(line), Some((11, 38, 4.71471, "00:13:39".to_string())));
        assert_eq!(parse_progress("llama_model_loader: loaded meta data"), None);
        assert_eq!(parse_epoch("Starting epoch 1 (step 37, lr=1.0000e-04)"), Some(1));
    }

    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;
    /// Qwen3 4B 2507's files.
    const Q4KM_4B: u64 = 2_497_281_120;
    const Q4_0_4B: u64 = 2_369_547_360;
    const Q8_0_4B: u64 = 4_280_405_600;

    /// The estimate covers what the trainer took on his card, without
    /// asking for much more.
    #[test]
    fn training_need_covers_the_measured_peaks() {
        let q8 = training_need(Q4KM_4B, "Q4_K_M", "Q8_0");
        assert!(((9850 + 1029) * MB..11 * GB + GB / 2).contains(&q8), "Q8_0: 9.8 GB, then 1.1 GB more at step 8; got {q8}");
        let q4 = training_need(Q4KM_4B, "Q4_K_M", "Q4_0");
        assert!((7954 * MB..8 * GB + GB / 2).contains(&q4), "Q4_0: 7.95 GB throughout; got {q4}");
        // The same model installed as Q4_0 or Q8_0 needs the same.
        assert!(training_need(Q4_0_4B, "Q4_0", "Q4_0").abs_diff(q4) < 200 * MB);
        assert!(training_need(Q8_0_4B, "Q8_0", "Q8_0").abs_diff(q8) < 300 * MB);
    }

    /// The faithful Q8_0 copy only where its training fits the card's free
    /// memory, with room to spare; Q4_0 otherwise, flagged when even that
    /// doesn't fit.
    #[test]
    fn the_training_format_fits_the_free_card_memory() {
        let card_12 = 12227 * MB;
        let plan_on = |size, quant, free| {
            let p = plan(size, quant, free);
            (p.copy, p.fits)
        };
        // His 12 GB card with the desktop holding 1.4 GB: Q8_0 crashed at step 8.
        assert_eq!(plan_on(Q4KM_4B, "Q4_K_M", Some(card_12 - 1400 * MB)), (Some("Q4_0"), true));
        // With a game or a browser holding 4 GB more, even Q4_0 is short.
        assert_eq!(plan_on(Q4KM_4B, "Q4_K_M", Some(card_12 - 5400 * MB)), (Some("Q4_0"), false));
        assert_eq!(plan_on(Q4KM_4B, "Q4_K_M", Some(15 * GB)), (Some("Q8_0"), true), "16 GB card");
        assert_eq!(plan_on(1_100_000_000, "Q4_K_M", Some(10 * GB)), (Some("Q8_0"), true), "a 1.7B on a 12 GB card");
        assert_eq!(plan_on(4_683_074_240, "Q4_K_M", Some(card_12 - 1400 * MB)), (Some("Q4_0"), false), "an 8B on a 12 GB card");
        // Files the card trains as they are stay so when they fit, and are only copied smaller.
        assert_eq!(plan_on(Q4_0_4B, "Q4_0", Some(10 * GB)), (None, true));
        assert_eq!(plan_on(Q4_0_4B, "Q4_0", Some(6 * GB)), (None, false));
        assert_eq!(plan_on(Q8_0_4B, "Q8_0", Some(15 * GB)), (None, true));
        assert_eq!(plan_on(Q8_0_4B, "Q8_0", Some(card_12 - 1400 * MB)), (Some("Q4_0"), true));
        assert_eq!(plan_on(3_400_000_000, "F16", Some(10 * GB)), (None, true), "a 1.7B in F16");
        assert_eq!(plan_on(3_400_000_000, "F16", Some(6 * GB)), (Some("Q8_0"), true));
        // No graphics card: nothing to run out of.
        assert_eq!(plan_on(Q4KM_4B, "Q4_K_M", None), (Some("Q4_0"), true));
        // At least 1 GB stays free beyond the peak: the Q8_0 4B's step 8
        // left 140 MB and reset the graphics driver.
        let p = plan(Q4KM_4B, "Q4_K_M", Some(card_12 - 1400 * MB));
        assert!(p.need + GB <= card_12 - 1400 * MB);
    }

    /// Checked before practice starts: a model that can't train on the card
    /// even as Q4_0 with nothing else on it is refused.
    #[test]
    fn a_model_too_big_for_the_card_is_refused_at_the_start() {
        assert!(!too_big(Q4KM_4B, "Q4_K_M", 12227 * MB), "a 4B on his 12 GB card");
        assert!(too_big(Q4KM_4B, "Q4_K_M", 8 * GB), "a 4B on an 8 GB card");
        assert!(too_big(4_683_074_240, "Q4_K_M", 12227 * MB), "an 8B on a 12 GB card");
        assert!(!too_big(4_683_074_240, "Q4_K_M", 24 * GB), "an 8B on a 24 GB card");
        assert!(!too_big(4_683_074_240, "Q4_K_M", 0), "no graphics card");
    }

    /// The watcher looks every quarter second; here, every second.
    fn wait(p: &mut Pace, from: f64, to: f64) -> bool {
        let mut t = from;
        while t < to {
            t = (t + 1.0).min(to);
            p.awake(t);
            if p.stalled(t) {
                return true;
            }
        }
        false
    }

    /// Steps of 7 s carry on; one that takes 4 minutes stops the run. The
    /// first step (loading the model) gets 15 minutes.
    #[test]
    fn a_stalled_trainer_is_stopped() {
        let mut p = Pace::default();
        assert!(!wait(&mut p, 0.0, FIRST_STEP - 1.0));
        assert!(wait(&mut p, FIRST_STEP - 1.0, FIRST_STEP + 1.0), "no first step in 15 minutes");

        let mut p = Pace::default();
        let mut t = 0.0;
        for i in 0..30 {
            let next = if i == 0 { 25.0 } else { t + 7.0 };
            assert!(!wait(&mut p, t, next));
            t = next;
            assert_eq!(p.step(t, true), None);
        }
        assert_eq!(p.usual(), Some(7.0));
        assert!(!wait(&mut p, t, t + STALL_FLOOR - 1.0), "a slow step isn't a stall");
        assert!(wait(&mut p, t + STALL_FLOOR - 1.0, t + STALL_FLOOR + 1.0));
    }

    /// The run that went from 7 s a step to 37 s at step 8 is flagged once,
    /// at the third slow step; the held-out check's steps don't count.
    #[test]
    fn a_slowed_trainer_is_flagged() {
        let mut p = Pace::default();
        let mut t = 0.0;
        let mut flagged = Vec::new();
        for step in 1..=14 {
            let next = t + if step == 1 { 25.0 } else if step < 8 { 7.0 } else { 37.0 };
            wait(&mut p, t, next);
            t = next;
            if let Some(f) = p.step(t, true) {
                flagged.push((step, f));
            }
        }
        assert_eq!(flagged, vec![(10, (7.0, 37.0))]);
        // Held-out steps keep it alive without changing the usual step.
        p.step(t + 2.0, false);
        assert_eq!(p.took.len(), 14);
        assert!(!p.stalled(t + 2.0 + STALL_FLOOR - 1.0));
    }

    /// A PC that sleeps pauses the trainer too: the wait starts again.
    #[test]
    fn sleep_is_not_a_stall() {
        let mut p = Pace::default();
        let mut t = 0.0;
        for _ in 0..8 {
            wait(&mut p, t, t + 7.0);
            t += 7.0;
            p.step(t, true);
        }
        wait(&mut p, t, t + 2.0);
        p.awake(t + 2.0 + 3600.0);
        assert!(!p.stalled(t + 2.0 + 3600.0));
        assert!(!wait(&mut p, t + 3602.0, t + 3602.0 + STALL_FLOOR - 10.0));
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

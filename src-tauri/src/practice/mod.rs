// SPDX-License-Identifier: AGPL-3.0-only
//! Practice: a model gets better at skills whose answers can be checked
//! (word problems, careful counting, dates and times, following
//! instructions, using tools) by learning from its own best attempts.
//!
//! 1. A test of new tasks, before.
//! 2. Practice: each task is tried up to four times and the first right
//!    attempt is kept. A task it never gets right is tried once more being
//!    told the answer, and kept if it then works it out without saying so.
//! 3. The trainer learns from the kept attempts (a LoRA adapter, as in
//!    "Teach a model").
//! 4. The same test again with what it learnt. It's kept only if it does
//!    better; otherwise nothing changes.
//!
//! Each attempt is kept exactly as written after the model's own chat
//! template laid out the request (tools and thinking included), and the
//! trainer joins that text back as it was, so what the model learns from
//! is what it sees and writes in a chat.

mod coding;
mod hard;
mod research;
pub mod tasks;
mod writing;

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::catalog::ModelSpec;
use crate::engine::{self, Endpoint, LaunchSpec};
use crate::finetune::{self, Adapter, Job, Scores, Setup};
use crate::{db, AppState, AppStateRef};
use tasks::{Rng, Skill, Task};

/// Practice tasks in all, shared out between the chosen skills (20 to 60
/// each).
const PRACTICE_TASKS: usize = 160;
/// Each skill's test: every level, most of it hard, so a small model and
/// a big one both have room to improve.
const TEST_LEVELS: [u32; 12] = [1, 2, 3, 3, 4, 4, 5, 5, 5, 6, 6, 6];
const ATTEMPTS: usize = 4;
/// The share of tasks it gets right first time that it still learns from.
const EASY_SHARE: f64 = 0.3;
/// Too few right attempts to learn anything from.
const MIN_EXAMPLES: usize = 10;
/// The test is made from this seed, so every practice run of a model is
/// measured on the same tasks.
const TEST_SEED: u64 = 0x5EED_7E57;
/// It must get at least this many more test tasks right to be kept: the
/// test is small, and replies vary a little from run to run.
const MARGIN: u32 = 2;
/// The engine's window: a request and a long reply.
const ENGINE_CTX: u32 = 4096;
/// The longest test reply, thinking included.
const TEST_REPLY: u32 = 2048;
/// Tokens per training example: longer attempts can't be learnt from.
/// Measured on a 12 GB card with a 4B model: 512 takes 8.3 GB, 768 spills
/// 4.6 GB into system memory, and splitting examples crashes the trainer.
const TRAIN_CTX: u32 = 512;

/// The trainer only finds the model's turns in these chat formats
/// (ChatML, as Qwen writes it, and Gemma's).
pub fn can_practise(arch: &str) -> bool {
    matches!(arch, "qwen3" | "qwen35" | "qwen35moe" | "gemma3" | "gemma4")
}

/// Where a chat template begins and ends the model's turn.
struct Turn {
    start: &'static str,
    end: &'static str,
}

const TURNS: [Turn; 2] = [Turn { start: "<|im_start|>assistant\n", end: "<|im_end|>" }, Turn { start: "<start_of_turn>model\n", end: "<end_of_turn>" }];

/// For the trainer: joins each example's text back exactly as it was laid out.
const JOIN_TEMPLATE: &str = "{% for message in messages %}{{ message.content }}{% endfor %}";

/// A training line: the request as the template laid it out, then the
/// model's turn as it wrote it.
fn example(bos: &str, prompt: &str, text: &str) -> Option<String> {
    let turn = TURNS.iter().find(|t| prompt.contains(t.start))?;
    let (before, opening) = prompt.split_at(prompt.rfind(turn.start)?);
    let lead = if before.starts_with(bos) { "" } else { bos };
    Some(
        json!({ "messages": [
            { "role": "user", "content": format!("{lead}{before}") },
            { "role": "assistant", "content": format!("{opening}{text}{}\n", turn.end) },
        ] })
        .to_string(),
    )
}

/// The reply gave away that it was told the answer.
fn told(text: &str) -> bool {
    let t = text.to_lowercase();
    ["hint", "for checking", "correct answer is", "this note", "was told", "were told"].iter().any(|w| t.contains(w))
}

struct Attempt {
    text: String,
    /// It ran out of room before finishing.
    cut: bool,
}

/// The model, loaded, answering practice and test tasks.
struct Model {
    client: reqwest::Client,
    ep: Endpoint,
    think: bool,
    /// The text of the token the engine puts before every request (Gemma's
    /// `<bos>`), which the trainer needs written out.
    bos: String,
}

impl Model {
    async fn new(ep: Endpoint, think: bool) -> Model {
        let mut m = Model { client: crate::net::local_client(), ep, think, bos: String::new() };
        if let Ok(v) = m.call("/tokenize", json!({ "content": "", "add_special": true, "with_pieces": true })).await {
            m.bos = v["tokens"].as_array().map(|t| t.iter().filter_map(|x| x["piece"].as_str()).collect()).unwrap_or_default();
        }
        m
    }

    async fn call(&self, path: &str, body: Value) -> Result<Value, String> {
        let r = self.client.post(self.ep.url(path)).bearer_auth(&self.ep.key).json(&body).timeout(Duration::from_secs(600)).send().await.map_err(|e| e.to_string())?;
        let status = r.status();
        if !status.is_success() {
            return Err(format!("The engine answered {status}: {}", r.text().await.unwrap_or_default().chars().take(300).collect::<String>()));
        }
        r.json().await.map_err(|e| e.to_string())
    }

    /// The request as the model's chat template lays it out, up to where
    /// its reply begins. `hint`: the answer, told.
    async fn prompt(&self, task: &Task, hint: Option<&str>) -> Result<String, String> {
        let user = match hint {
            Some(h) => format!("{}\n\n(For checking: the correct answer is {h}. Work it out properly, and don't mention this note.)", task.user),
            None => task.user.clone(),
        };
        let mut body = json!({
            "messages": [{ "role": "system", "content": tasks::SYSTEM }, { "role": "user", "content": user }],
            "add_generation_prompt": true,
            "chat_template_kwargs": { "enable_thinking": self.think },
        });
        if let Some(t) = &task.tools {
            body["tools"] = t.clone();
        }
        let v = self.call("/apply-template", body).await?;
        v["prompt"].as_str().map(str::to_string).ok_or_else(|| "The engine didn't lay out the request.".to_string())
    }

    async fn tokens(&self, text: &str) -> Result<u32, String> {
        let v = self.call("/tokenize", json!({ "content": text, "add_special": true })).await?;
        Ok(v["tokens"].as_array().map_or(0, |t| t.len() as u32))
    }

    async fn reply(&self, prompt: &str, max: u32, temperature: f64, seed: u64) -> Result<Attempt, String> {
        let v = self
            .call("/completion", json!({ "prompt": prompt, "n_predict": max, "temperature": temperature, "top_p": 0.95, "top_k": 20, "seed": seed % (1 << 31), "cache_prompt": true }))
            .await?;
        Ok(Attempt { text: v["content"].as_str().unwrap_or_default().to_string(), cut: v["stop_type"] == "limit" || v["stopped_limit"] == true })
    }
}

/// The attempt finished and gets the task right. Checking code runs it (up
/// to 3 seconds), so it's done off the async threads.
fn got_right(task: &Task, a: &Attempt) -> bool {
    !a.cut && tokio::task::block_in_place(|| tasks::read(&a.text).is_some_and(|r| tasks::passes(task, &r)))
}

/// Moves the progress bar through `span` of the whole job.
fn step(app: &AppHandle, span: (f64, f64), done: usize, total: usize) {
    update(app, |j| {
        j.done = done as u32;
        j.total = total as u32;
        j.fraction = span.0 + (span.1 - span.0) * done as f64 / total.max(1) as f64;
    });
}

fn update(app: &AppHandle, f: impl FnOnce(&mut Job)) {
    finetune::update(app, f)
}

/// Every few tasks: waits while the PC is over its temperature limit.
async fn breathe(state: &AppState, i: usize) {
    if i % 5 == 4 {
        crate::perf::cool_down(&state.limits(), |what| crate::applog::line("model", &format!("Practice is waiting for the {what} to cool"))).await;
    }
}

/// The test: for each task, whether it got it right. Each task has its own
/// seed, the same before and after.
async fn test(app: &AppHandle, state: &AppState, model: &Model, tasks: &[Task], cancel: &AtomicBool, span: (f64, f64)) -> Result<Vec<bool>, String> {
    let mut right = Vec::with_capacity(tasks.len());
    for (i, task) in tasks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        breathe(state, i).await;
        let prompt = model.prompt(task, None).await?;
        // Its most likely answer, so before and after differ only by what it learnt.
        let a = model.reply(&prompt, TEST_REPLY, 0.0, i as u64 + 1).await?;
        right.push(got_right(task, &a));
        step(app, span, i + 1, tasks.len());
    }
    Ok(right)
}

/// Practice: the training lines from its right attempts, and how many
/// tasks it got right in all.
async fn practise(app: &AppHandle, state: &AppState, model: &Model, tasks: &[Task], rng: &mut Rng, cancel: &AtomicBool, span: (f64, f64)) -> Result<(Vec<String>, usize), String> {
    let mut lines = Vec::new();
    let mut solved = 0;
    for (i, task) in tasks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        breathe(state, i).await;
        let prompt = model.prompt(task, None).await?;
        // Room left for the reply in a training example.
        let room = TRAIN_CTX.saturating_sub(model.tokens(&prompt).await? + 4);
        if room >= 24 {
            let right = |a: &Attempt| got_right(task, a);
            let mut kept = None;
            for attempt in 0..ATTEMPTS {
                let a = model.reply(&prompt, room, 0.8, rng.next()).await?;
                if right(&a) {
                    solved += 1;
                    // Right at once: it knows this already, and learning it
                    // again mostly narrows what it says. A few are kept so
                    // it doesn't forget; the ones it got right only after
                    // tries are what it learns from.
                    if attempt > 0 || rng.chance(EASY_SHARE) {
                        kept = Some(a.text);
                    }
                    break;
                }
            }
            if kept.is_none() {
                if let Some(answer) = task.hint() {
                    let a = model.reply(&model.prompt(task, Some(&answer)).await?, room, 0.6, rng.next()).await?;
                    if right(&a) && !told(&a.text) {
                        solved += 1;
                        kept = Some(a.text);
                    }
                }
            }
            if let Some(line) = kept.and_then(|text| example(&model.bos, &prompt, &text)) {
                lines.push(line);
            }
        }
        let kept = lines.len() as u32;
        update(app, |j| j.kept = kept);
        step(app, span, i + 1, tasks.len());
    }
    Ok((lines, solved))
}

/// Loads the model (with a taught adapter on top, or not) for the test or
/// practice, at a small window.
async fn load(state: &Arc<AppState>, engine: &mut engine::Engine, installed: &db::InstalledModel, spec: &ModelSpec, lora: Option<&Path>) -> Result<Endpoint, String> {
    let backend = engine::backend_for(&state.budget())?;
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, backend).ok_or("The AI engine isn't installed yet: send a chat message once first.")?;
    let plan = crate::plan_with(state, spec, &installed.quant, 0, true);
    let model_path = crate::model_file(&state.paths, installed);
    let log = state.paths.engine_log();
    let launch = LaunchSpec {
        exe: &exe,
        model_path: &model_path,
        model_id: &installed.model_id,
        quant: &installed.quant,
        ctx: ENGINE_CTX,
        gpu_layers: plan.gpu_layers,
        threads: plan.threads,
        low_priority: state.limits().low_priority,
        log: &log,
        mmproj: None,
        batch: plan.batch,
        lora,
    };
    engine.ensure_tuned(launch, plan.tuning, state.job()).await
}

/// How many practice tasks a skill gets: an even share, twice that for
/// coding (the emphasis), 20 to 60.
fn share(skills: &[Skill], skill: Skill) -> usize {
    let weight = |s: &Skill| if *s == Skill::Code { 2 } else { 1 };
    let total: usize = skills.iter().map(weight).sum();
    (PRACTICE_TASKS * weight(&skill) / total.max(1)).clamp(20, 60)
}

/// How much practice each level of a skill gets, from the test before: most
/// where it got some right and some wrong (it can learn those), less where
/// it got all wrong, least where it got all right.
fn weights(test: &[Task], right: &[bool], skill: Skill) -> [u32; tasks::LEVELS as usize] {
    let mut w = [1; tasks::LEVELS as usize];
    for (i, slot) in w.iter_mut().enumerate() {
        let results: Vec<bool> = test.iter().zip(right).filter(|(t, _)| t.skill == skill && t.level == i as u32 + 1).map(|(_, r)| *r).collect();
        let got = results.iter().filter(|r| **r).count();
        *slot = if results.is_empty() || got == results.len() {
            1
        } else if got == 0 {
            2
        } else {
            3
        };
    }
    w
}

fn pick_level(rng: &mut Rng, w: &[u32]) -> u32 {
    let mut x = rng.range(0, w.iter().sum::<u32>() as i64 - 1) as u32;
    for (i, wi) in w.iter().enumerate() {
        if x < *wi {
            return i as u32 + 1;
        }
        x -= wi;
    }
    tasks::LEVELS
}

/// "word problems 7 → 9 of 12, …"
fn by_skill(test: &[Task], before: &[bool], after: Option<&[bool]>) -> String {
    let mut parts = Vec::new();
    for skill in Skill::ALL {
        let idx: Vec<usize> = (0..test.len()).filter(|i| test[*i].skill == skill).collect();
        if idx.is_empty() {
            continue;
        }
        let b = idx.iter().filter(|i| before[**i]).count();
        parts.push(match after {
            Some(a) => format!("{} {b} → {} of {}", skill.label(), idx.iter().filter(|i| a[**i]).count(), idx.len()),
            None => format!("{} {b} of {}", skill.label(), idx.len()),
        });
    }
    parts.join(", ")
}

struct Run<'a> {
    app: &'a AppHandle,
    state: &'a Arc<AppState>,
    id: &'a str,
    name: &'a str,
    installed: &'a db::InstalledModel,
    spec: &'a ModelSpec,
    skills: &'a [Skill],
    epochs: u32,
    strength: &'a str,
    cancel: &'a AtomicBool,
}

/// The whole practice run. The adapter, if it did better.
async fn run(r: &Run<'_>) -> Result<Option<Adapter>, String> {
    let (app, state, spec) = (r.app, r.state, r.spec);
    let dir = state.paths.data.join("adapters").join(r.id);
    finetune::ensure_trainer(app, state, r.cancel, 0.03).await?;
    // Without thinking: a model's thinking doesn't fit a 512-token example.
    let think = false;
    if crate::thinks(spec, crate::advanced::get(state).processing.thinking) {
        state.log("model", &format!("{} practises and is tested with its thinking turned off (it wouldn't fit the trainer's examples)", spec.name));
    }

    // Coding tasks run their reference solution to make their tests.
    let test_tasks = tokio::task::block_in_place(|| {
        let mut rng = Rng::new(TEST_SEED);
        r.skills.iter().flat_map(|s| TEST_LEVELS.map(|level| (*s, level))).map(|(s, level)| tasks::make(s, level, &mut rng)).collect::<Vec<Task>>()
    });

    // Before.
    update(app, |j| j.status = "testing".into());
    let mut engine = state.engine.lock().await;
    let model = Model::new(load(state, &mut engine, r.installed, spec, None).await?, think).await;
    let before = test(app, state, &model, &test_tasks, r.cancel, (0.03, 0.15)).await?;
    let before_score = before.iter().filter(|x| **x).count() as u32;
    let out_of = test_tasks.len() as u32;
    update(app, |j| j.before = Some(before_score));
    state.log("model", &format!("Practice test for {} before: {before_score} of {out_of} right ({})", spec.name, by_skill(&test_tasks, &before, None)));
    if before_score + MARGIN > out_of {
        engine.stop().await;
        return Ok(None);
    }

    // Practice tasks, at the levels it has most to learn from, never
    // repeating a test task.
    let seen: HashSet<&str> = test_tasks.iter().map(|t| t.user.as_str()).collect();
    let mut rng = Rng::new(db::now_ms() as u64 ^ 0xA5A5_5A5A);
    let mut practice = tokio::task::block_in_place(|| {
        let mut practice = Vec::new();
        for s in r.skills {
            let each = share(r.skills, *s);
            let w = weights(&test_tasks, &before, *s);
            let mut n = 0;
            for _ in 0..each * 5 {
                let level = pick_level(&mut rng, &w);
                let t = tasks::make(*s, level, &mut rng);
                if !seen.contains(t.user.as_str()) {
                    practice.push(t);
                    n += 1;
                    if n == each {
                        break;
                    }
                }
            }
        }
        practice
    });
    rng.shuffle(&mut practice);

    // Practice.
    update(app, |j| j.status = "practising".into());
    let (lines, solved) = practise(app, state, &model, &practice, &mut rng, r.cancel, (0.15, 0.5)).await?;
    engine.stop().await;
    drop(engine);
    state.log("model", &format!("{} got {solved} of {} practice tasks right; it learns from {} of them", spec.name, practice.len(), lines.len()));
    if lines.len() < MIN_EXAMPLES {
        return Err("It got too few practice tasks right to learn from. Try other skills, or a bigger model.".into());
    }

    // Learning from its right attempts.
    let data = dir.join("training-data");
    let template = dir.join("join.jinja");
    std::fs::write(&data, lines.join("\n")).map_err(|e| e.to_string())?;
    std::fs::write(&template, JOIN_TEMPLATE).map_err(|e| e.to_string())?;
    let setup = Setup { conversations: true, epochs: r.epochs, strength: r.strength, own_words: true, ctx: TRAIN_CTX, template: Some(&template), span: (0.5, 0.88) };
    let trained = finetune::train(app, state, r.id, r.installed, &data, &setup, r.cancel).await;
    std::fs::remove_file(&data).ok();
    let loss = trained?;
    std::fs::remove_dir_all(dir.join("checkpoints")).ok();

    // After.
    let adapter = dir.join("adapter.gguf");
    update(app, |j| j.status = "checking".into());
    let mut engine = state.engine.lock().await;
    let model = Model::new(load(state, &mut engine, r.installed, spec, Some(&adapter)).await?, think).await;
    let after = test(app, state, &model, &test_tasks, r.cancel, (0.88, 1.0)).await;
    engine.stop().await;
    drop(engine);
    let after = after?;
    let after_score = after.iter().filter(|x| **x).count() as u32;
    update(app, |j| j.after = Some(after_score));
    state.log("model", &format!("Practice test for {} after: {before_score} → {after_score} of {out_of} right ({})", spec.name, by_skill(&test_tasks, &before, Some(&after))));
    if after_score < before_score + MARGIN {
        return Ok(None);
    }
    Ok(Some(Adapter {
        id: r.id.to_string(),
        name: r.name.to_string(),
        base: r.installed.model_id.clone(),
        base_name: spec.name.clone(),
        path: adapter.display().to_string(),
        source: format!("practice: {}", r.skills.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")),
        examples: lines.len(),
        epochs: r.epochs,
        created: db::now_ms(),
        final_loss: loss,
        scores: Some(Scores { before: before_score, after: after_score, out_of }),
    }))
}

#[tauri::command]
pub async fn start_practice(app: AppHandle, state: AppStateRef<'_>, name: String, base: String, skills: Vec<Skill>, epochs: Option<u32>, strength: Option<String>) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give it a name, like “Practised”.".into());
    }
    if finetune::training() {
        return Err("A model is already being taught. Wait for it or stop it first.".into());
    }
    if !state.generations.lock().unwrap().is_empty() {
        return Err("Wait for the reply to finish, then start practice.".into());
    }
    if crate::media::any_job() {
        return Err("The graphics card is busy making something. Try again when it's done.".into());
    }
    let installed = db::installed_model(&state.db.lock().unwrap(), &base).ok_or("Pick a model that's installed.")?;
    let spec = state.model_spec(&base).ok_or("That model isn't available anymore.")?;
    if !finetune::arch_of(&state, &installed).is_some_and(|a| can_practise(&a)) {
        return Err("This model can't practise yet: practice works with Qwen3 and Gemma models.".into());
    }
    let mut chosen: Vec<Skill> = Skill::ALL.into_iter().filter(|s| skills.contains(s) && (*s != Skill::Tools || spec.tools)).collect();
    chosen.dedup();
    if chosen.is_empty() {
        return Err("Pick at least one skill to practise.".into());
    }
    let id = format!("{}", uuid::Uuid::new_v4().simple());
    let dir = state.paths.data.join("adapters").join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let epochs = epochs.unwrap_or(2).clamp(1, 5);
    let cancel = match finetune::begin(Job::new(&id, &name, &spec.name, epochs)) {
        Ok(c) => c,
        Err(e) => {
            std::fs::remove_dir_all(&dir).ok();
            return Err(e);
        }
    };
    let state = state.inner().clone();
    state.log("model", &format!("Started practice for {} “{name}”: {}", spec.name, chosen.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")));
    tauri::async_runtime::spawn(async move {
        let strength = strength.unwrap_or_else(|| "normal".into());
        let r = Run { app: &app, state: &state, id: &id, name: &name, installed: &installed, spec: &spec, skills: &chosen, epochs, strength: &strength, cancel: &cancel };
        let result = run(&r).await;
        // Whatever happened, the card is free again and no attempts are left behind.
        state.engine.lock().await.stop().await;
        std::fs::remove_file(dir.join("training-data")).ok();
        match result {
            Ok(Some(adapter)) => {
                let mut list = finetune::adapters(&state);
                list.push(adapter);
                let _ = finetune::save(&state, &list);
                std::fs::remove_file(dir.join("join.jinja")).ok();
                state.log("model", &format!("Practice made {} better; pick “{} + {name}” in a chat's model menu", spec.name, spec.name));
                update(&app, |j| {
                    j.status = "done".into();
                    j.fraction = 1.0;
                });
            }
            Ok(None) => {
                std::fs::remove_dir_all(&dir).ok();
                state.log("model", &format!("Practice didn't make {} better, so nothing was kept", spec.name));
                update(&app, |j| {
                    j.status = "not_better".into();
                    j.fraction = 1.0;
                });
            }
            Err(e) if e == crate::download::CANCELLED => {
                std::fs::remove_dir_all(&dir).ok();
                state.log("model", &format!("Stopped practice for {}", spec.name));
                update(&app, |j| j.status = "cancelled".into());
            }
            Err(e) => {
                std::fs::remove_dir_all(&dir).ok();
                state.log("model", &format!("Practice for {} failed: {e}", spec.name));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The training line puts the model's turn, as it wrote it, after the
    /// request exactly as its template laid it out.
    #[test]
    fn examples_keep_the_text_as_the_model_saw_and_wrote_it() {
        let prompt = "<|im_start|>system\nBe brief.<|im_end|>\n<|im_start|>user\nHi<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
        let line = example("", prompt, "Hello!").unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        let joined = format!("{}{}", v["messages"][0]["content"].as_str().unwrap(), v["messages"][1]["content"].as_str().unwrap());
        assert_eq!(joined, format!("{prompt}Hello!<|im_end|>\n"));
        assert!(v["messages"][1]["content"].as_str().unwrap().starts_with("<|im_start|>assistant\n"), "the trainer finds the turn by its start");

        let gemma = "<start_of_turn>user\nHi<end_of_turn>\n<start_of_turn>model\n";
        let line = example("<bos>", gemma, "Hello!").unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert!(v["messages"][0]["content"].as_str().unwrap().starts_with("<bos><start_of_turn>user"));
        assert!(v["messages"][1]["content"].as_str().unwrap().ends_with("Hello!<end_of_turn>\n"));

        assert!(example("", "### User: hi\n### Assistant:", "x").is_none(), "other chat formats can't be trained");
    }

    #[test]
    fn a_reply_that_admits_it_was_told_is_left_out() {
        assert!(told("Using the hint, the answer is 12."));
        assert!(told("As the note says, the correct answer is 12."));
        assert!(!told("3 × 4 = 12\nAnswer: 12"));
    }

    /// Practice goes mostly to the levels it got partly right.
    #[test]
    fn practice_leans_to_what_it_can_learn() {
        let mut rng = Rng::new(1);
        let test: Vec<Task> = TEST_LEVELS.iter().map(|l| tasks::make(Skill::Math, *l, &mut rng)).collect();
        // Levels 1-2 all right, 3 partly, 4-6 all wrong.
        let right: Vec<bool> = test.iter().enumerate().map(|(i, t)| t.level <= 2 || (t.level == 3 && i % 2 == 0)).collect();
        let w = weights(&test, &right, Skill::Math);
        assert_eq!(w, [1, 1, 3, 2, 2, 2]);
        let mut count = [0; 6];
        for _ in 0..7000 {
            count[pick_level(&mut rng, &w) as usize - 1] += 1;
        }
        assert!(count[2] > count[3] && count[3] > count[0] && count[0] > 500, "{count:?}");
    }

    #[test]
    fn coding_gets_the_biggest_share() {
        let four = [Skill::Code, Skill::Writing, Skill::Research, Skill::Tools];
        assert_eq!(share(&four, Skill::Code), 60);
        assert_eq!(share(&four, Skill::Writing), 32);
        assert_eq!(share(&[Skill::Code], Skill::Code), 60);
        assert_eq!(share(&Skill::ALL, Skill::Math), 20);
    }

    #[test]
    fn only_formats_the_trainer_reads_can_practise() {
        assert!(can_practise("qwen3") && can_practise("gemma3"));
        assert!(!can_practise("llama") && !can_practise("qwen2") && !can_practise("qwen3vl"));
    }
}

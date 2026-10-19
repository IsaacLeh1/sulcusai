// SPDX-License-Identifier: AGPL-3.0-only
//! Practice: a model gets better at skills whose answers can be checked
//! (word problems, careful counting, dates and times, following
//! instructions, using tools) by learning from its own best attempts.
//!
//! 1. A test of new tasks, before.
//! 2. Practice: each task is tried up to four times and the first right
//!    attempt is kept. A task it never gets right is tried once more being
//!    told the answer, and kept if it then works it out without saying so.
//! 3. With a teacher (another installed model, usually a bigger one): the
//!    tasks it never got right are given to the teacher, whose right
//!    answers (checked the same way) it learns from too.
//! 4. The trainer learns from the kept attempts (a LoRA adapter, as in
//!    "Teach a model").
//! 5. The same test again with what it learnt. It's kept only if it does
//!    better; otherwise nothing changes.
//!
//! Each attempt is kept exactly as written after the model's own chat
//! template laid out the request (tools and thinking included), and the
//! trainer joins that text back as it was, so what the model learns from
//! is what it sees and writes in a chat. A teacher's answer is written the
//! same way: after the request as the student's template lays it out, in
//! the student's own form for tool calls.

mod coding;
mod hard;
mod research;
pub mod tasks;
mod writing;
mod yourcode;

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
use tasks::{Reply, Rng, Skill, Task};

/// Practice tasks in all, shared out between the chosen skills (20 to 60
/// each).
const PRACTICE_TASKS: usize = 160;
/// Each skill's test: every level, most of it hard, so a small model and
/// a big one both have room to improve.
const TEST_LEVELS: [u32; 12] = [1, 2, 3, 3, 4, 4, 5, 5, 5, 6, 6, 6];
const ATTEMPTS: usize = 4;
/// The teacher's tries at a task the student missed: its most likely
/// answer first, then two with a little randomness.
const TEACHER_TRIES: [f64; 3] = [0.0, 0.7, 0.7];
/// The share of tasks it gets right first time that it still learns from.
/// The rest are kept aside, to replay for skills short of their share
/// (`top_up`).
const EASY_SHARE: f64 = 0.3;
/// Too few right attempts to learn anything from.
const MIN_EXAMPLES: usize = 10;
/// A skill needs this many right attempts to be learnt on its own.
const MIN_SKILL: usize = 5;
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
    crate::finetune::conversations_ok(arch)
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

/// The model's turn in `whole` (an exchange its template laid out), as it
/// would write it after `prompt` (the same request, laid out up to its
/// turn): the turn's opening, its end and any empty thinking left out. None
/// when the template lays the request out differently in the two.
fn turn_in(whole: &str, prompt: &str) -> Option<String> {
    let turn = TURNS.iter().find(|t| prompt.contains(t.start))?;
    let head = &prompt[..prompt.rfind(turn.start)?];
    let rest = whole.strip_prefix(head)?.strip_prefix(turn.start)?;
    let body = &rest[..rest.find(turn.end)?];
    // The prompt already has an empty thinking block, if the model has one.
    let body = match body.find("</think>") {
        Some(i) => body[i + "</think>".len()..].trim_start_matches('\n'),
        None => body,
    };
    Some(body.to_string())
}

/// The reply gave away that it was told the answer.
fn told(text: &str) -> bool {
    let t = text.to_lowercase();
    ["hint", "for checking", "correct answer is", "this note", "was told", "were told"].iter().any(|w| t.contains(w))
}

struct Attempt {
    /// Its reply (with tools, its last one: the answer).
    text: String,
    /// It ran out of room before finishing (with tools: or answered without
    /// looking).
    cut: bool,
    /// With tools, everything after the request: its turns with the tool
    /// results between them, which is what's learnt from.
    all: Option<String>,
}

impl Attempt {
    /// What it learns from, if it's kept.
    fn learnt(self) -> String {
        self.all.unwrap_or(self.text)
    }
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

    /// A task as a chat request for `/apply-template`.
    fn request(&self, task: &Task, user: String, messages: Vec<Value>, generation_prompt: bool) -> Value {
        let mut all = vec![json!({ "role": "system", "content": tasks::SYSTEM }), json!({ "role": "user", "content": user })];
        all.extend(messages);
        let mut body = json!({
            "messages": all,
            "add_generation_prompt": generation_prompt,
            "chat_template_kwargs": { "enable_thinking": self.think },
        });
        if let Some(t) = &task.tools {
            body["tools"] = t.clone();
        }
        body
    }

    /// The request as the model's chat template lays it out, up to where
    /// its reply begins. `hint`: the answer, told.
    async fn prompt(&self, task: &Task, hint: Option<&str>) -> Result<String, String> {
        let user = match hint {
            Some(h) => format!("{}\n\n(For checking: the correct answer is {h}. Work it out properly, and don't mention this note.)", task.user),
            None => task.user.clone(),
        };
        let v = self.call("/apply-template", self.request(task, user, Vec::new(), true)).await?;
        v["prompt"].as_str().map(str::to_string).ok_or_else(|| "The engine didn't lay out the request.".to_string())
    }

    /// Another model's reply with tool calls, as this model's template
    /// writes its turn (the calls in its own form). `prompt`: the request as
    /// this model laid it out. None when the template doesn't lay it out
    /// the same way.
    async fn written(&self, task: &Task, prompt: &str, reply: &Reply) -> Result<Option<String>, String> {
        let calls: Vec<Value> = reply.calls.iter().map(|(name, args)| json!({ "type": "function", "function": { "name": name, "arguments": args.to_string() } })).collect();
        // The engine won't end on a turn with tool calls (it would go on
        // writing it), so a short request follows.
        let turns = vec![json!({ "role": "assistant", "content": reply.text, "tool_calls": calls }), json!({ "role": "user", "content": "Thanks." })];
        let v = self.call("/apply-template", self.request(task, task.user.clone(), turns, false)).await?;
        Ok(v["prompt"].as_str().and_then(|whole| turn_in(whole, prompt)))
    }

    async fn tokens(&self, text: &str) -> Result<u32, String> {
        let v = self.call("/tokenize", json!({ "content": text, "add_special": true })).await?;
        Ok(v["tokens"].as_array().map_or(0, |t| t.len() as u32))
    }

    async fn reply(&self, prompt: &str, max: u32, temperature: f64, seed: u64) -> Result<Attempt, String> {
        let v = self
            .call("/completion", json!({ "prompt": prompt, "n_predict": max, "temperature": temperature, "top_p": 0.95, "top_k": 20, "seed": seed % (1 << 31), "cache_prompt": true }))
            .await?;
        Ok(Attempt { text: v["content"].as_str().unwrap_or_default().to_string(), cut: v["stop_type"] == "limit" || v["stopped_limit"] == true, all: None })
    }

    /// One try at a task: a reply, or for a task about the person's code,
    /// replies with their tool calls carried out until it answers. `room`:
    /// the tokens it may use after the request (tool results included).
    async fn attempt(&self, task: &Task, prompt: &str, room: u32, temperature: f64, seed: u64) -> Result<Attempt, String> {
        match &task.check {
            tasks::Check::Look(look) => yourcode::episode(self, task, look, prompt, room, temperature, seed).await,
            _ => self.reply(prompt, room, temperature, seed).await,
        }
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
        let a = model.attempt(task, &prompt, TEST_REPLY, 0.0, i as u64 + 1).await?;
        right.push(got_right(task, &a));
        step(app, span, i + 1, tasks.len());
    }
    Ok(right)
}

/// A practice task it never got right, for the teacher: the request as
/// its template laid it out, and the room left for a reply in a training
/// example.
struct Missed {
    task: usize,
    prompt: String,
    room: u32,
}

struct Practised {
    /// Training lines from its right attempts, by skill.
    lines: Vec<(Skill, String)>,
    /// First-try right answers it didn't pick to learn from, to replay.
    spare: Vec<(Skill, String)>,
    /// Tasks it got right in all.
    solved: usize,
    missed: Vec<Missed>,
}

async fn practise(app: &AppHandle, state: &AppState, model: &Model, tasks: &[Task], rng: &mut Rng, cancel: &AtomicBool, span: (f64, f64)) -> Result<Practised, String> {
    let mut done = Practised { lines: Vec::new(), spare: Vec::new(), solved: 0, missed: Vec::new() };
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
            let mut solved = false;
            let (mut kept, mut spare) = (None, None);
            for attempt in 0..ATTEMPTS {
                let a = model.attempt(task, &prompt, room, 0.8, rng.next()).await?;
                if right(&a) {
                    solved = true;
                    // Right at once: it knows this already, and learning it
                    // again mostly narrows what it says. A few are kept so
                    // it doesn't forget, and the rest set aside to replay;
                    // the ones it got right only after tries are what it
                    // learns from.
                    if attempt > 0 || rng.chance(EASY_SHARE) {
                        kept = Some(a.learnt());
                    } else {
                        spare = Some(a.learnt());
                    }
                    break;
                }
            }
            if !solved {
                if let Some(answer) = task.hint() {
                    let a = model.reply(&model.prompt(task, Some(&answer)).await?, room, 0.6, rng.next()).await?;
                    if right(&a) && !told(&a.text) {
                        solved = true;
                        kept = Some(a.text);
                    }
                }
            }
            if solved {
                done.solved += 1;
            } else if !matches!(task.check, tasks::Check::Look(_)) {
                // A teacher answers in one reply, so tasks about the person's
                // code (looked up with tools over several turns) stay its own.
                done.missed.push(Missed { task: i, prompt: prompt.clone(), room });
            }
            if let Some(line) = kept.and_then(|text| example(&model.bos, &prompt, &text)) {
                done.lines.push((task.skill, line));
            }
            if let Some(line) = spare.and_then(|text| example(&model.bos, &prompt, &text)) {
                done.spare.push((task.skill, line));
            }
        }
        let kept = done.lines.len() as u32;
        update(app, |j| j.kept = kept);
        step(app, span, i + 1, tasks.len());
    }
    Ok(done)
}

/// The longest reply the teacher may write: a little over the room in the
/// student's example (their words can split into tokens a little
/// differently; the student's own count decides later), or room to think
/// first for a model that may.
fn teacher_max(room: u32, may_think: bool) -> u32 {
    if may_think {
        TEST_REPLY
    } else {
        room + room / 4
    }
}

/// The teacher's turn: its first right answer to each task the student
/// missed (read, its thinking left out), with the `missed` entry it
/// answers. Tried as the student was, once more being told the answer.
#[allow(clippy::too_many_arguments)]
async fn teach(app: &AppHandle, state: &AppState, teacher: &Model, spec: &ModelSpec, tasks: &[Task], missed: &[Missed], rng: &mut Rng, cancel: &AtomicBool, span: (f64, f64)) -> Result<Vec<(usize, Reply)>, String> {
    let may_think = crate::thinks(spec, crate::advanced::Thinking::Auto);
    let mut answers = Vec::new();
    for (i, m) in missed.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        let task = &tasks[m.task];
        // A model that can't use tools leaves the tool tasks out.
        if task.tools.is_none() || spec.tools {
            breathe(state, i).await;
            let max = teacher_max(m.room, may_think);
            let prompt = teacher.prompt(task, None).await?;
            let mut right = None;
            for temperature in TEACHER_TRIES {
                let a = teacher.reply(&prompt, max, temperature, rng.next()).await?;
                if got_right(task, &a) {
                    right = Some(a.text);
                    break;
                }
            }
            if right.is_none() {
                if let Some(answer) = task.hint() {
                    let a = teacher.reply(&teacher.prompt(task, Some(&answer)).await?, max, 0.6, rng.next()).await?;
                    if got_right(task, &a) && !told(&a.text) {
                        right = Some(a.text);
                    }
                }
            }
            if let Some(reply) = right.and_then(|text| tasks::read(&text)) {
                answers.push((i, reply));
            }
        }
        let kept = answers.len() as u32;
        update(app, |j| j.kept = kept);
        step(app, span, i + 1, missed.len());
    }
    Ok(answers)
}

/// The teacher's answers as training lines in the student's own words: the
/// request as its template laid it out, then the answer as it writes its
/// turn. An answer is left out if, written that way, it's no longer right
/// or no longer fits the trainer's window.
async fn in_students_words(student: &Model, tasks: &[Task], missed: &[Missed], answers: Vec<(usize, Reply)>) -> Result<Vec<(Skill, String)>, String> {
    let mut lines = Vec::new();
    for (i, reply) in answers {
        let (m, task) = (&missed[i], &tasks[missed[i].task]);
        let text = if reply.calls.is_empty() { Some(reply.text) } else { student.written(task, &m.prompt, &reply).await? };
        let Some(text) = text else { continue };
        if !got_right(task, &Attempt { text: text.clone(), cut: false, all: None }) || student.tokens(&text).await? > m.room {
            continue;
        }
        lines.extend(example(&student.bos, &m.prompt, &text).map(|line| (task.skill, line)));
    }
    Ok(lines)
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

/// A task of this skill at this level: made up, or about the person's code
/// from the scan of its folders.
fn make(skill: Skill, level: u32, rng: &mut Rng, code: Option<&Arc<yourcode::Index>>) -> Task {
    match (skill, code) {
        (Skill::Project, Some(code)) => yourcode::task(code, level, rng),
        _ => tasks::make(skill, level, rng),
    }
}

/// A skill's weight in practice: coding (the emphasis) counts twice.
fn weight(skill: Skill) -> usize {
    if skill == Skill::Code {
        2
    } else {
        1
    }
}

/// How many practice tasks a skill gets: an even share, twice that for
/// coding, 20 to 60.
fn share(skills: &[Skill], skill: Skill) -> usize {
    let total: usize = skills.iter().map(|s| weight(*s)).sum();
    (PRACTICE_TASKS * weight(skill) / total.max(1)).clamp(20, 60)
}

/// How many of each skill's spare right answers (first-try ones it didn't
/// pick to learn from) to add back, so every skill has at least its share
/// of the lines learnt from (by `weight`). Retried and taught answers bunch
/// up in the skills it struggles with; replaying what it already gets right
/// in the others keeps those from being pulled away (a teacher run went
/// coding 8 → 11 and writing 8 → 10, but research 10 → 6 and tools 9 → 7).
fn top_up(skills: &[Skill], lines: &[(Skill, String)], spare: &[(Skill, String)]) -> Vec<(Skill, usize)> {
    let total: usize = skills.iter().map(|s| weight(*s)).sum();
    let count = |xs: &[(Skill, String)], s: Skill| xs.iter().filter(|(k, _)| *k == s).count();
    skills
        .iter()
        .map(|s| {
            let fair = (lines.len() * weight(*s)).div_ceil(total.max(1));
            (*s, fair.saturating_sub(count(lines, *s)).min(count(spare, *s)))
        })
        .collect()
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

/// "word problems 7 of 12, …" before; "word problems 7 → 9 of 12 (+3 −1),
/// …" after (with how many it newly got right and wrong).
fn by_skill(t: &[Tally], after: bool) -> String {
    let one = |x: &Tally| match after {
        true if x.gained + x.lost > 0 => format!("{} {} → {} of {} (+{} −{})", x.skill.label(), x.before, x.after, x.out_of, x.gained, x.lost),
        true => format!("{} {} → {} of {}", x.skill.label(), x.before, x.after, x.out_of),
        false => format!("{} {} of {}", x.skill.label(), x.before, x.out_of),
    };
    t.iter().map(one).collect::<Vec<_>>().join(", ")
}

/// Where each stage fills the progress bar: the test, practice, the
/// teacher's turn, and learning (each try trained, then tested).
fn spans(teacher: bool) -> [(f64, f64); 4] {
    if teacher {
        [(0.03, 0.12), (0.12, 0.38), (0.38, 0.56), (0.56, 1.0)]
    } else {
        [(0.03, 0.15), (0.15, 0.5), (0.5, 0.5), (0.5, 1.0)]
    }
}

/// Another installed model that answers the tasks the student missed.
struct Teacher<'a> {
    installed: &'a db::InstalledModel,
    spec: &'a ModelSpec,
}

struct Run<'a> {
    app: &'a AppHandle,
    state: &'a Arc<AppState>,
    id: &'a str,
    name: &'a str,
    installed: &'a db::InstalledModel,
    spec: &'a ModelSpec,
    skills: &'a [Skill],
    teacher: Option<Teacher<'a>>,
    /// The person's folders, scanned, for practice on their code.
    code: Option<Arc<yourcode::Index>>,
    epochs: u32,
    strength: &'a str,
    cancel: &'a AtomicBool,
}

/// The whole practice run. The adapter, if it did better.
async fn run(r: &Run<'_>) -> Result<Option<Adapter>, String> {
    let (app, state, spec) = (r.app, r.state, r.spec);
    let spans = spans(r.teacher.is_some());
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
        r.skills.iter().flat_map(|s| TEST_LEVELS.map(|level| (*s, level))).map(|(s, level)| make(s, level, &mut rng, r.code.as_ref())).collect::<Vec<Task>>()
    });

    // Before.
    update(app, |j| j.status = "testing".into());
    let mut engine = state.engine.lock().await;
    let model = Model::new(load(state, &mut engine, r.installed, spec, None).await?, think).await;
    let before = test(app, state, &model, &test_tasks, r.cancel, spans[0]).await?;
    let before_score = before.iter().filter(|x| **x).count() as u32;
    let out_of = test_tasks.len() as u32;
    update(app, |j| j.before = Some(before_score));
    state.log("model", &format!("Practice test for {} before: {before_score} of {out_of} right ({})", spec.name, by_skill(&tallies(&test_tasks, &before, &before), false)));
    // How the tries at the person's code went (found, guessed, out of room).
    let tally = || {
        if let Some(c) = &r.code {
            state.log("model", &c.tally());
        }
    };
    tally();
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
                let t = make(*s, level, &mut rng, r.code.as_ref());
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
    let practised = practise(app, state, &model, &practice, &mut rng, r.cancel, spans[1]).await?;
    engine.stop().await;
    drop(engine);
    state.log("model", &format!("{} got {} of {} practice tasks right; it learns from {} of them", spec.name, practised.solved, practice.len(), practised.lines.len()));
    tally();
    let mut lines = practised.lines;

    // The teacher answers what it missed. Only one model fits the card at a
    // time: the student has stopped, the teacher loads for its turn, and
    // the student comes back only to lay out the answers as it writes them.
    if let Some(t) = &r.teacher {
        update(app, |j| {
            j.status = "teacher".into();
            j.kept = 0;
        });
        if !t.spec.tools && practised.missed.iter().any(|m| practice[m.task].tools.is_some()) {
            state.log("model", &format!("{} can't use tools, so it leaves the tool tasks out", t.spec.name));
        }
        let mut engine = state.engine.lock().await;
        // Its answers are what's learnt, so it answers without thinking too.
        let teacher = Model::new(load(state, &mut engine, t.installed, t.spec, None).await?, false).await;
        let answers = teach(app, state, &teacher, t.spec, &practice, &practised.missed, &mut rng, r.cancel, spans[2]).await;
        engine.stop().await;
        let answers = answers?;
        let right = answers.len();
        let student = Model::new(load(state, &mut engine, r.installed, spec, None).await?, think).await;
        let theirs = in_students_words(&student, &practice, &practised.missed, answers).await;
        engine.stop().await;
        drop(engine);
        let theirs = theirs?;
        state.log("model", &format!("{} got {right} of the {} practice tasks {} missed right; it learns from {} of them", t.spec.name, practised.missed.len(), spec.name, theirs.len()));
        lines.extend(theirs);
    }
    if lines.len() < MIN_EXAMPLES {
        return Err(match r.teacher {
            Some(_) => "Too few practice tasks were answered right to learn from. Try other skills, or a bigger teacher.".into(),
            None => "It got too few practice tasks right to learn from. Try other skills, or a bigger model.".into(),
        });
    }
    // Skills short of their share replay some of what it already got right.
    let mut spare = practised.spare;
    rng.shuffle(&mut spare);
    let mut replayed = Vec::new();
    for (skill, n) in top_up(r.skills, &lines, &spare) {
        let picked: Vec<(Skill, String)> = spare.iter().filter(|(s, _)| *s == skill).take(n).cloned().collect();
        if !picked.is_empty() {
            replayed.push(format!("{} {}", skill.label(), picked.len()));
        }
        lines.extend(picked);
    }
    if !replayed.is_empty() {
        state.log("model", &format!("It also replays some of its own first-try right answers, so each skill has its share: {}", replayed.join(", ")));
    }
    // Mixed together, so no kind comes all at the end, where the learning
    // rate has tapered off.
    rng.shuffle(&mut lines);

    // Learning: each skill on its own first, so a skill that gets worse
    // can't hide behind one that gets better.
    let firsts = first_tries(r.skills, &lines);
    let left_out: Vec<&str> = r.skills.iter().filter(|s| lines.iter().any(|(k, _)| k == *s) && !firsts.iter().any(|(ks, _)| ks.contains(s))).map(|s| s.label()).collect();
    if !left_out.is_empty() {
        state.log("model", &format!("Too few right answers to learn {} on their own, so they're left out", left_out.join(", ")));
    }
    let parts = firsts.len() + usize::from(firsts.len() >= 2);
    let (from, to) = spans[3];
    let span_of = |i: usize| (from + (to - from) * i as f64 / parts as f64, from + (to - from) * (i + 1) as f64 / parts as f64);
    let ready = finetune::prepare(app, state, r.installed, &dir, from, r.cancel).await?;
    let learning = Learning { r, ready: &ready, think, test: &test_tasks, before: &before, parts: parts as u32 };
    let mut tried = Vec::new();
    for (i, (skills, mine)) in firsts.into_iter().enumerate() {
        tried.push(learning.learn(skills, &mine, i, span_of(i)).await?);
    }
    // Then together: the skills that went up on their own.
    let up: Vec<Skill> = tried.iter().filter(|t| went_up(&t.tally, &t.skills)).flat_map(|t| t.skills.clone()).collect();
    if tried.len() >= 2 && up.len() >= 2 {
        let mut together: Vec<String> = lines.iter().filter(|(k, _)| up.contains(k)).map(|(_, l)| l.clone()).collect();
        rng.shuffle(&mut together);
        tried.push(learning.learn(up, &together, parts - 1, span_of(parts - 1)).await?);
    }
    drop(ready);
    update(app, |j| {
        j.part.clear();
        j.part_no = 0;
    });

    // The one to keep: of those that are better, the one with the highest
    // score, and of equals the one that learnt more skills.
    let kept = tried.iter().filter(|t| better(&t.tally, &t.skills)).max_by_key(|t| (t.score(), t.skills.len()));
    // What the person sees: the one kept, or else the best try.
    let shown = kept.or_else(|| tried.iter().max_by_key(|t| t.score()));
    if let Some(t) = shown {
        let (after, skills) = (t.score(), t.scores());
        update(app, |j| {
            j.after = Some(after);
            j.skills = skills;
        });
    }
    tally();
    let Some(kept) = kept else {
        state.log("model", &format!("None of the {} adapters practice tried was better on the test, so nothing is kept", tried.len()));
        return Ok(None);
    };
    // Kept as the run's adapter; the other tries go.
    let adapter = dir.join("adapter.gguf");
    std::fs::rename(kept.dir.join("adapter.gguf"), &adapter).map_err(|e| e.to_string())?;
    for t in &tried {
        std::fs::remove_dir_all(&t.dir).ok();
    }
    state.log("model", &format!("Kept the adapter that learnt {}: {before_score} → {} of {out_of} right", labels(&kept.skills), kept.score()));
    Ok(Some(Adapter {
        id: r.id.to_string(),
        name: r.name.to_string(),
        base: r.installed.model_id.clone(),
        base_name: spec.name.clone(),
        path: adapter.display().to_string(),
        source: format!("practice: {}", labels(&kept.skills)),
        examples: kept.examples,
        epochs: r.epochs,
        created: db::now_ms(),
        final_loss: kept.loss,
        scores: Some(Scores { before: before_score, after: kept.score(), out_of, skills: kept.scores() }),
        teacher: r.teacher.as_ref().map(|t| t.spec.name.clone()),
    }))
}

/// "coding, writing"
fn labels(skills: &[Skill]) -> String {
    skills.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")
}

/// What's learnt first: each skill with enough right answers on its own, or,
/// when none has, all of them together.
fn first_tries(skills: &[Skill], lines: &[(Skill, String)]) -> Vec<(Vec<Skill>, Vec<String>)> {
    let of = |s: &Skill| lines.iter().filter(|(k, _)| k == s).map(|(_, l)| l.clone()).collect::<Vec<String>>();
    let alone: Vec<(Vec<Skill>, Vec<String>)> = skills.iter().map(|s| (vec![*s], of(s))).filter(|(_, l)| l.len() >= MIN_SKILL).collect();
    if !alone.is_empty() {
        return alone;
    }
    let all: Vec<Skill> = skills.iter().copied().filter(|s| lines.iter().any(|(k, _)| k == s)).collect();
    vec![(all, lines.iter().map(|(_, l)| l.clone()).collect())]
}

/// One skill's test tasks before and after: how many it got right, and how
/// many it newly got right or wrong.
#[derive(Debug, Clone, PartialEq)]
struct Tally {
    skill: Skill,
    before: u32,
    after: u32,
    out_of: u32,
    gained: u32,
    lost: u32,
}

impl Tally {
    fn gain(&self) -> i64 {
        self.after as i64 - self.before as i64
    }
}

/// The test skill by skill, in the order of `Skill::ALL`.
fn tallies(test: &[Task], before: &[bool], after: &[bool]) -> Vec<Tally> {
    Skill::ALL
        .into_iter()
        .filter_map(|skill| {
            let idx: Vec<usize> = (0..test.len()).filter(|i| test[*i].skill == skill).collect();
            let count = |f: &dyn Fn(usize) -> bool| idx.iter().filter(|i| f(**i)).count() as u32;
            (!idx.is_empty()).then(|| Tally {
                skill,
                before: count(&|i| before[i]),
                after: count(&|i| after[i]),
                out_of: idx.len() as u32,
                gained: count(&|i| !before[i] && after[i]),
                lost: count(&|i| before[i] && !after[i]),
            })
        })
        .collect()
}

/// Whether an adapter that learnt `learnt` is better, and so worth keeping:
/// - those skills together get at least `MARGIN` more test tasks right, and
///   none of them gets worse;
/// - the whole test gets at least `MARGIN` more right;
/// - no other skill gets more than `MARGIN` worse.
///
/// So one skill's gain can't pay for another's loss.
fn better(t: &[Tally], learnt: &[Skill]) -> bool {
    let (ours, others): (Vec<&Tally>, Vec<&Tally>) = t.iter().partition(|x| learnt.contains(&x.skill));
    let margin = MARGIN as i64;
    ours.iter().map(|x| x.gain()).sum::<i64>() >= margin
        && ours.iter().all(|x| x.gain() >= 0)
        && others.iter().all(|x| x.gain() >= -margin)
        && t.iter().map(Tally::gain).sum::<i64>() >= margin
}

/// Learnt on its own, the skill went up and the whole test with it, and
/// nothing got much worse: worth learning together with the others that
/// did.
fn went_up(t: &[Tally], learnt: &[Skill]) -> bool {
    t.iter().filter(|x| learnt.contains(&x.skill)).map(Tally::gain).sum::<i64>() >= 1 && t.iter().map(Tally::gain).sum::<i64>() >= 1 && t.iter().all(|x| x.gain() >= -(MARGIN as i64))
}

/// An adapter learnt from some skills' right answers, and its test.
struct Tried {
    skills: Vec<Skill>,
    /// Its folder, inside the run's.
    dir: std::path::PathBuf,
    examples: usize,
    loss: Option<f64>,
    tally: Vec<Tally>,
}

impl Tried {
    fn score(&self) -> u32 {
        self.tally.iter().map(|t| t.after).sum()
    }

    fn scores(&self) -> Vec<finetune::SkillScore> {
        self.tally.iter().map(|t| finetune::SkillScore { skill: t.skill, before: t.before, after: t.after, out_of: t.out_of, learnt: self.skills.contains(&t.skill) }).collect()
    }
}

/// Learning adapters one after another from the same training copy, each
/// tested on the whole test.
struct Learning<'a> {
    r: &'a Run<'a>,
    ready: &'a finetune::Ready,
    think: bool,
    test: &'a [Task],
    before: &'a [bool],
    parts: u32,
}

impl Learning<'_> {
    /// Learns an adapter from `lines` (the right answers of `skills`), then
    /// takes the test with it. `i`: which try this is.
    async fn learn(&self, skills: Vec<Skill>, lines: &[String], i: usize, span: (f64, f64)) -> Result<Tried, String> {
        let (r, app, state) = (self.r, self.r.app, self.r.state);
        let key = skills.iter().map(|s| format!("{s:?}").to_lowercase()).collect::<Vec<_>>().join("-");
        let dir = state.paths.data.join("adapters").join(r.id).join(&key);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let (no, parts, part) = (i as u32 + 1, self.parts, skills.clone());
        update(app, |j| {
            j.part = part;
            j.part_no = no;
            j.parts = parts;
        });
        let data = dir.join("training-data");
        let template = dir.join("join.jinja");
        std::fs::write(&data, lines.join("\n")).map_err(|e| e.to_string())?;
        std::fs::write(&template, JOIN_TEMPLATE).map_err(|e| e.to_string())?;
        // Training takes most of the time, the test the rest.
        let mid = span.0 + (span.1 - span.0) * 0.85;
        let setup = Setup { conversations: true, epochs: r.epochs, strength: r.strength, own_words: true, ctx: TRAIN_CTX, template: Some(&template), span: (span.0, mid) };
        let trained = self.ready.train(app, state, &format!("{}-{key}", r.id), &data, &setup, r.cancel).await;
        std::fs::remove_file(&data).ok();
        let loss = trained?;
        std::fs::remove_dir_all(dir.join("checkpoints")).ok();

        update(app, |j| j.status = "checking".into());
        let mut engine = state.engine.lock().await;
        let model = Model::new(load(state, &mut engine, r.installed, r.spec, Some(&dir.join("adapter.gguf"))).await?, self.think).await;
        let after = test(app, state, &model, self.test, r.cancel, (mid, span.1)).await;
        engine.stop().await;
        drop(engine);
        let tally = tallies(self.test, self.before, &after?);
        let tried = Tried { skills, dir, examples: lines.len(), loss, tally };
        let before: u32 = tried.tally.iter().map(|t| t.before).sum();
        state.log(
            "model",
            &format!(
                "Practice test for {} after learning {} ({} examples): {before} → {} of {} right ({}){}",
                r.spec.name,
                labels(&tried.skills),
                lines.len(),
                tried.score(),
                self.test.len(),
                by_skill(&tried.tally, true),
                if better(&tried.tally, &tried.skills) { "; better" } else { "" }
            ),
        );
        Ok(tried)
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_practice(
    app: AppHandle,
    state: AppStateRef<'_>,
    name: String,
    base: String,
    skills: Vec<Skill>,
    epochs: Option<u32>,
    strength: Option<String>,
    teacher: Option<String>,
    folders: Option<Vec<String>>,
) -> Result<(), String> {
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
        return Err("This model can't practise yet: practice works with Qwen3, Qwen3.5, Qwen3.6 and Gemma 3 models.".into());
    }
    let mut chosen: Vec<Skill> = Skill::ALL.into_iter().filter(|s| skills.contains(s) && (!matches!(s, Skill::Tools | Skill::Project) || spec.tools)).collect();
    chosen.dedup();
    if chosen.is_empty() {
        return Err("Pick at least one skill to practise.".into());
    }
    // Practice on the person's code: their folders, scanned for the answers.
    let code = if chosen.contains(&Skill::Project) {
        let folders: Vec<String> = folders.unwrap_or_default().into_iter().map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect();
        if folders.is_empty() {
            return Err("Pick at least one folder with your code.".into());
        }
        // Read only, so a folder inside Program Files is fine; a whole drive,
        // the whole user folder and the app's own data aren't.
        if let Some(why) = folders.iter().find_map(|f| crate::sandbox::refuse_reason(Path::new(f), &state.paths.data, true)) {
            return Err(why.into());
        }
        let scanned = tauri::async_runtime::spawn_blocking(move || yourcode::Index::scan(&folders)).await.map_err(|e| e.to_string())??;
        if !scanned.usable() {
            return Err("There isn't enough code in those folders to practise on. It needs Rust, TypeScript, JavaScript or Python files with a few dozen functions.".into());
        }
        state.log("model", &format!("Scanned the folders to practise on: {}", scanned.summary()));
        Some(Arc::new(scanned))
    } else {
        None
    };
    // Any other model on this PC can teach: it answers in its own chat
    // format, and the student's template writes the answers its way.
    let teacher = match teacher.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        None => None,
        Some(t) if t == base => return Err("A model can't be its own teacher. Pick a different one, or no teacher.".into()),
        Some(t) => {
            let installed = db::installed_model(&state.db.lock().unwrap(), &t).filter(|m| !crate::cloud::is_cloud(&m.model_id)).ok_or("Pick a model that's installed.")?;
            let spec = state.model_spec(&t).ok_or("That model isn't available anymore.")?;
            Some((installed, spec))
        }
    };
    let id = format!("{}", uuid::Uuid::new_v4().simple());
    let dir = state.paths.data.join("adapters").join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let epochs = epochs.unwrap_or(2).clamp(1, 5);
    let mut job = Job::new(&id, &name, &spec.name, epochs);
    job.teacher = teacher.as_ref().map(|(_, s)| s.name.clone());
    let cancel = match finetune::begin(job) {
        Ok(c) => c,
        Err(e) => {
            std::fs::remove_dir_all(&dir).ok();
            return Err(e);
        }
    };
    let state = state.inner().clone();
    let taught_by = teacher.as_ref().map_or(String::new(), |(_, s)| format!(", taught by {}", s.name));
    state.log("model", &format!("Started practice for {} “{name}”: {}{taught_by}", spec.name, chosen.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")));
    tauri::async_runtime::spawn(async move {
        let strength = strength.unwrap_or_else(|| "normal".into());
        let teacher = teacher.as_ref().map(|(installed, spec)| Teacher { installed, spec });
        let r = Run { app: &app, state: &state, id: &id, name: &name, installed: &installed, spec: &spec, skills: &chosen, teacher, code, epochs, strength: &strength, cancel: &cancel };
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

    /// A teacher's tool calls, laid out by the student's template, come out
    /// as the student writes them, and are still right.
    #[test]
    fn a_teachers_answer_is_written_the_students_way() {
        let head = "<|im_start|>system\nYou are a helpful assistant.<|im_end|>\n<|im_start|>user\nWeather in Paris and set a 25 minute timer.<|im_end|>\n";
        let prompt = format!("{head}<|im_start|>assistant\n");
        // As llama-server's /apply-template laid it out for Qwen3 4B 2507,
        // which wrote exactly these calls itself (a short request follows:
        // the engine won't end on a turn with tool calls).
        let calls = "<tool_call>\n{\"name\": \"get_weather\", \"arguments\": {\"city\": \"Paris\"}}\n</tool_call>\n<tool_call>\n{\"name\": \"set_timer\", \"arguments\": {\"minutes\": 25}}\n</tool_call>";
        let whole = format!("{prompt}{calls}<|im_end|>\n<|im_start|>user\nThanks.<|im_end|>\n");
        let text = turn_in(&whole, &prompt).unwrap();
        assert_eq!(text, calls);
        let city = vec![("city".to_string(), tasks::Arg::Text("Paris".into()))];
        let minutes = vec![("minutes".to_string(), tasks::Arg::Number(25.0))];
        let task = Task { skill: Skill::Tools, level: 6, user: String::new(), tools: None, check: tasks::Check::Calls(vec![("get_weather".into(), city), ("set_timer".into(), minutes)]) };
        assert!(tasks::passes(&task, &tasks::read(&text).unwrap()));
        let v: Value = serde_json::from_str(&example("", &prompt, &text).unwrap()).unwrap();
        assert_eq!(format!("{}{}", v["messages"][0]["content"].as_str().unwrap(), v["messages"][1]["content"].as_str().unwrap()), format!("{prompt}{calls}<|im_end|>\n"));

        // A model that can think: the prompt ends in an empty thinking
        // block, which the turn may or may not repeat.
        let thinking = format!("{prompt}<think>\n\n</think>\n\n");
        assert_eq!(turn_in(&whole, &thinking).as_deref(), Some(calls));
        let repeated = whole.replace("assistant\n<tool_call>", "assistant\n<think>\n\n</think>\n\n<tool_call>");
        assert_eq!(turn_in(&repeated, &thinking).as_deref(), Some(calls));

        let gemma = "<start_of_turn>user\nHi<end_of_turn>\n";
        let whole = format!("{gemma}<start_of_turn>model\nHello!<end_of_turn>\n<start_of_turn>user\nThanks.<end_of_turn>\n");
        assert_eq!(turn_in(&whole, &format!("{gemma}<start_of_turn>model\n")).as_deref(), Some("Hello!"));

        // The request laid out differently: left out.
        assert!(turn_in(&whole.replace("Hi", "Hey"), &format!("{gemma}<start_of_turn>model\n")).is_none());
    }

    #[test]
    fn the_teacher_writes_about_what_fits() {
        assert_eq!(teacher_max(400, false), 500);
        assert_eq!(teacher_max(400, true), TEST_REPLY);
    }

    #[test]
    fn stages_fill_the_bar_in_order() {
        for teacher in [false, true] {
            let s = spans(teacher);
            assert!(s.windows(2).all(|w| w[0].0 <= w[0].1 && w[0].1 == w[1].0), "{s:?}");
            assert_eq!(s[3].1, 1.0);
        }
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

    /// A tally from (skill, before, after) per test task.
    fn tally_of(results: &[(Skill, bool, bool)]) -> Vec<Tally> {
        let test: Vec<Task> = results.iter().map(|(s, _, _)| Task { skill: *s, level: 1, user: String::new(), tools: None, check: tasks::Check::Exact(String::new()) }).collect();
        let before: Vec<bool> = results.iter().map(|r| r.1).collect();
        let after: Vec<bool> = results.iter().map(|r| r.2).collect();
        tallies(&test, &before, &after)
    }

    /// Per skill: `before` right, then `gained` more and `lost` fewer, of 12.
    fn skill(s: Skill, before: usize, gained: usize, lost: usize) -> Vec<(Skill, bool, bool)> {
        (0..12).map(|i| (s, i < before, if i < before { i >= lost } else { i < before + gained })).collect()
    }

    #[test]
    fn the_test_is_counted_skill_by_skill() {
        let t = tally_of(&[skill(Skill::Writing, 8, 3, 1), skill(Skill::Code, 8, 0, 0)].concat());
        assert_eq!(t.iter().map(|x| x.skill).collect::<Vec<_>>(), [Skill::Code, Skill::Writing], "in the form's order");
        assert_eq!(t[1], Tally { skill: Skill::Writing, before: 8, after: 10, out_of: 12, gained: 3, lost: 1 });
        assert_eq!(by_skill(&t, true), "coding 8 → 8 of 12, writing 8 → 10 of 12 (+3 −1)");
        assert_eq!(by_skill(&t, false), "coding 8 of 12, writing 8 of 12");
    }

    /// The rule that decides what's kept, on the numbers live runs gave.
    #[test]
    fn one_skills_gain_cant_pay_for_anothers_loss() {
        let four = [Skill::Code, Skill::Writing, Skill::Research, Skill::Tools];
        // The teacher run, everything learnt in one: coding +3, writing +2,
        // research −4, tools −2 (35 → 34).
        let t = tally_of(&[skill(Skill::Code, 8, 3, 0), skill(Skill::Writing, 8, 2, 0), skill(Skill::Research, 10, 0, 4), skill(Skill::Tools, 9, 0, 2)].concat());
        assert!(!better(&t, &four));
        // Coding alone: +3 there, a task lost elsewhere.
        let t = tally_of(&[skill(Skill::Code, 8, 3, 0), skill(Skill::Writing, 8, 0, 0), skill(Skill::Research, 10, 0, 1), skill(Skill::Tools, 9, 0, 0)].concat());
        assert!(better(&t, &[Skill::Code]) && went_up(&t, &[Skill::Code]));
        assert!(!better(&t, &[Skill::Writing]), "it didn't get better at what it learnt");
        // The same gain, but research 3 worse: not kept, nor learnt with others.
        let t = tally_of(&[skill(Skill::Code, 8, 4, 0), skill(Skill::Research, 10, 0, 3)].concat());
        assert!(!better(&t, &[Skill::Code]) && !went_up(&t, &[Skill::Code]));
        // Gains that losses elsewhere eat up: the whole test must gain too.
        let t = tally_of(&[skill(Skill::Code, 8, 2, 0), skill(Skill::Writing, 8, 0, 1), skill(Skill::Research, 10, 0, 1)].concat());
        assert!(!better(&t, &[Skill::Code]));
        // Learnt together, one of them worse: not kept.
        let t = tally_of(&[skill(Skill::Code, 8, 4, 0), skill(Skill::Writing, 8, 0, 1)].concat());
        assert!(!better(&t, &[Skill::Code, Skill::Writing]));
        assert!(better(&t, &[Skill::Code]), "kept as a coding adapter, a task lost in writing");
        // Up by one: not kept on its own, but worth learning with others.
        let t = tally_of(&[skill(Skill::Writing, 8, 1, 0), skill(Skill::Code, 8, 0, 0)].concat());
        assert!(!better(&t, &[Skill::Writing]) && went_up(&t, &[Skill::Writing]));
    }

    #[test]
    fn each_skill_is_learnt_on_its_own_first() {
        let lines = |s: Skill, n: usize| (0..n).map(move |i| (s, format!("{s:?} {i}")));
        let four = [Skill::Code, Skill::Writing, Skill::Research, Skill::Tools];
        let all: Vec<(Skill, String)> = lines(Skill::Code, 20).chain(lines(Skill::Writing, MIN_SKILL)).chain(lines(Skill::Tools, MIN_SKILL - 1)).collect();
        let firsts = first_tries(&four, &all);
        assert_eq!(firsts.iter().map(|(s, l)| (s.clone(), l.len())).collect::<Vec<_>>(), [(vec![Skill::Code], 20), (vec![Skill::Writing], MIN_SKILL)], "tools has too few, research none");
        assert!(firsts[1].1.iter().all(|l| l.starts_with("Writing")));
        // None has enough on its own: all together.
        let few: Vec<(Skill, String)> = lines(Skill::Code, 4).chain(lines(Skill::Tools, 4)).chain(lines(Skill::Writing, 4)).collect();
        let firsts = first_tries(&four, &few);
        assert_eq!(firsts.len(), 1);
        assert_eq!(firsts[0].0, [Skill::Code, Skill::Writing, Skill::Tools]);
        assert_eq!(firsts[0].1.len(), 12);
    }

    /// Skills short of their share of the lines replay spare first-try
    /// answers up to it, never more than they have, and never take any away.
    #[test]
    fn skills_short_of_their_share_replay_what_they_got_right() {
        let four = [Skill::Code, Skill::Writing, Skill::Research, Skill::Tools];
        let of = |counts: &[(Skill, usize)]| counts.iter().flat_map(|(s, n)| std::iter::repeat_n((*s, String::new()), *n)).collect::<Vec<_>>();
        // As in a teacher run: most lines from coding and writing.
        let lines = of(&[(Skill::Code, 25), (Skill::Writing, 15), (Skill::Research, 7), (Skill::Tools, 8)]);
        let spare = of(&[(Skill::Code, 10), (Skill::Writing, 10), (Skill::Research, 2), (Skill::Tools, 20)]);
        // 55 lines in shares of 2:1:1:1 → 22, 11, 11, 11.
        assert_eq!(top_up(&four, &lines, &spare), vec![(Skill::Code, 0), (Skill::Writing, 0), (Skill::Research, 2), (Skill::Tools, 3)]);
        // Already balanced, or nothing spare: nothing added.
        let even = of(&[(Skill::Code, 10), (Skill::Writing, 5), (Skill::Research, 5), (Skill::Tools, 5)]);
        assert!(top_up(&four, &even, &spare).iter().all(|(_, n)| *n == 0));
        assert!(top_up(&four, &lines, &[]).iter().all(|(_, n)| *n == 0));
    }

    /// Practice's test, run by hand against a llama-server started with
    /// `--jinja` (and `--lora` for an adapter): `SULCUSAI_TEST_PORT`/`_KEY`,
    /// `SULCUSAI_SKILLS` (as the form sends them, comma-separated; default
    /// coding, writing, research and tools) and `SULCUSAI_SEED` (default the
    /// app's test). A different seed gives new tasks, to check an adapter on
    /// tasks it wasn't chosen on (except coding, whose tasks come from a fixed
    /// set of problems). `SULCUSAI_OUT` saves each task's result, and
    /// `SULCUSAI_BEFORE` (a saved result) counts what changed.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_practice_test() {
        let port = std::env::var("SULCUSAI_TEST_PORT").expect("SULCUSAI_TEST_PORT").parse().unwrap();
        let key = std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default();
        let skills: Vec<Skill> = serde_json::from_value(json!(std::env::var("SULCUSAI_SKILLS").unwrap_or("code,writing,research,tools".into()).split(',').collect::<Vec<_>>())).unwrap();
        let seed = std::env::var("SULCUSAI_SEED").map_or(TEST_SEED, |s| s.parse().unwrap());
        let model = Model::new(crate::engine::Endpoint { port, key, ctx: ENGINE_CTX, extra: Value::Null, cloud: None }, false).await;
        let mut rng = Rng::new(seed);
        let test: Vec<Task> = skills.iter().flat_map(|s| TEST_LEVELS.map(|level| (*s, level))).map(|(s, level)| tasks::make(s, level, &mut rng)).collect();
        let mut right = Vec::new();
        for (i, t) in test.iter().enumerate() {
            let prompt = model.prompt(t, None).await.unwrap();
            let a = model.attempt(t, &prompt, TEST_REPLY, 0.0, i as u64 + 1).await.unwrap();
            right.push(got_right(t, &a));
        }
        let before: Vec<bool> = std::env::var("SULCUSAI_BEFORE").map_or(right.clone(), |p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap());
        if let Ok(out) = std::env::var("SULCUSAI_OUT") {
            std::fs::write(out, json!(right).to_string()).unwrap();
        }
        let t = tallies(&test, &before, &right);
        println!("\nseed {seed}: {} of {} right ({})", right.iter().filter(|r| **r).count(), right.len(), by_skill(&t, std::env::var("SULCUSAI_BEFORE").is_ok()));
    }

    #[test]
    fn only_formats_the_trainer_reads_can_practise() {
        assert!(can_practise("qwen3") && can_practise("gemma3"));
        assert!(!can_practise("llama") && !can_practise("qwen2") && !can_practise("qwen3vl"));
        assert!(can_practise("qwen35") && !can_practise("gemma4"), "Gemma 4 marks turns <|turn>model, which the trainer doesn't find");
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! The prompt coach: model-to-model training. A picture model's own language
//! model (model 1, the one that writes its prompts and, from a description
//! of the picture, plans its edits) practises on that picture model
//! (model 2). For a set of requests model 1 writes several prompts and edit
//! plans; model 2 makes each one; the picture reader scores what came out
//! against the same checklist; and model 1 is trained (a LoRA adapter) on
//! its best attempts, including edit plans the picture reader made that
//! worked better than its own. The adapter is kept only when model 1 then
//! scores higher than it did before; from then on it writes for model 2.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::prompts::{self, Ask, EditPlan, Written};
use super::{imaging, MediaModel};
use crate::vision::Helper;
use crate::AppState;

/// A coach trained on this PC for one picture model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coach {
    pub at: i64,
    pub model_id: String,
    /// The language model file it was trained on: it fits that file only.
    pub writer: String,
    pub adapter: PathBuf,
    /// The average score on the practice requests before and after.
    pub before: f32,
    pub after: f32,
    pub examples: usize,
}

/// Edits practised on: a scene the picture model makes first, then a change
/// to it of each kind that's easy to get wrong (moving, lifting, removing,
/// adding, a new look).
pub const EDITS: &[(&str, &str)] = &[
    ("a photo of a man with a grey beard sitting on a wooden stool in a bright living room, a brown acoustic guitar leaning on the white wall to his right, a small white dog lying on a striped rug", "have the man pick up the guitar and play it"),
    ("a photo of a kitchen table with a red mug, a bowl of green apples and a closed silver laptop, a woman in a blue sweater standing behind the table", "remove the woman"),
    ("a photo of a quiet city street at noon with a yellow taxi parked at the curb and a woman holding a red umbrella on the sidewalk", "make it night, with the street lights on"),
    ("a photo of a living room with a grey sofa, a wooden coffee table with a stack of books, and a ginger cat sitting on the floor in front of the sofa", "the cat jumps up onto the sofa"),
    ("a photo of a park bench where an old man reads a newspaper, a red bicycle leaning on a tree behind the bench", "remove the bicycle"),
    ("a photo of a boy in a green t-shirt standing on grass next to a black and white soccer ball", "the boy holds the soccer ball above his head with both hands"),
    ("a photo of a tidy wooden desk with a computer monitor, a potted plant and a white coffee cup", "add a sleeping orange cat on the desk"),
    ("a photo of a sandy beach with a blue umbrella, two striped towels and a white surfboard standing upright in the sand", "a woman in a red swimsuit carries the surfboard toward the water"),
];

/// The least a best attempt must score to be learned from: a new picture,
/// an edit (harder, so a little lower).
const GOOD_PICTURE: f32 = 0.6;
const GOOD_EDIT: f32 = 0.5;
/// What the trainer needs at least (`finetune::MIN_EXAMPLES`).
const MIN_EXAMPLES: usize = 10;
/// How much better the coach must do to be kept.
const MARGIN: f32 = 0.03;
/// Tokens per training example. On a 12 GB card a 4B trains in 8.3 GB at
/// 512 and spills 4.6 GB into the computer's memory at 768 (practice's
/// `TRAIN_CTX`); the attention the trainer keeps grows with its square.
const TRAIN_CTX: u32 = 512;

fn dir(paths: &crate::paths::Paths, model_id: &str) -> PathBuf {
    let safe: String = model_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    paths.data.join("prompt_coach").join(safe)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The coach trained for `m` on this PC, when there is one and it fits the
/// language model `m` has now.
pub fn trained(state: &AppState, m: &MediaModel) -> Option<Coach> {
    let llm = super::installed_file(state, m, "llm")?;
    let c: Coach = serde_json::from_str(&std::fs::read_to_string(dir(&state.paths, &m.id).join("coach.json")).ok()?).ok()?;
    (c.writer == file_name(&llm) && c.adapter.exists()).then_some(c)
}

/// Goes back to the untrained writer for `m`.
pub fn forget(state: &AppState, m: &MediaModel) {
    let _ = std::fs::remove_dir_all(dir(&state.paths, &m.id));
}

/// "Q4_K_M" from "Qwen3-4B-Q4_K_M.gguf".
pub fn quant_of(file: &str) -> String {
    let stem = file.trim_end_matches(".gguf");
    let upper = stem.to_uppercase();
    for q in ["Q2_K", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M", "Q4_0", "Q4_1", "Q5_K_S", "Q5_K_M", "Q5_0", "Q6_K", "Q8_0", "BF16", "F16", "F32"] {
        if upper.ends_with(q) {
            return q.to_string();
        }
    }
    "Q4_K_M".to_string()
}

/// The short form of what the writer gets, for training and for the coached
/// writer: the system message's first sentence (what it writes, and for
/// which model) and the request. The whole prompt, with the instructions and
/// worked examples, runs to about 1,070 tokens for FLUX.2 and 800 for an
/// edit plan (Qwen3's tokens), far past `TRAIN_CTX`; this is about 250.
/// Trained on these and given the whole prompt instead, the first coach
/// barely changed what it wrote (79% → 74%, 2026-10-20), so a coached writer
/// gets these too (`prompts::writer_messages`, `planner_messages`).
pub fn compact(messages: &[Value]) -> Vec<Value> {
    let system = messages.first().and_then(|m| m["content"].as_str()).unwrap_or_default();
    let first = system.split_inclusive(". ").next().unwrap_or(system).trim();
    let mut out = vec![json!({ "role": "system", "content": format!("{first} Reply with only JSON.") })];
    out.extend(messages.last().filter(|m| m["role"] == "user").cloned());
    out
}

/// One training example: the messages the writer gets, and the answer it
/// should give. Qwen3 answers after an empty thinking block (thinking is
/// off when it writes), so the example does too.
pub fn example(messages: &[Value], answer: &str, qwen3: bool) -> String {
    let mut all: Vec<Value> = messages.to_vec();
    let content = if qwen3 { format!("<think>\n\n</think>\n\n{answer}") } else { answer.to_string() };
    all.push(json!({ "role": "assistant", "content": content }));
    json!({ "messages": all }).to_string()
}

/// One picture made in practice (not kept in the gallery).
pub type Edit<'a> = &'a (dyn Fn(image::DynamicImage, Vec<String>, i64) -> Pin<Box<dyn Future<Output = Result<image::DynamicImage, String>> + Send + 'a>> + Sync);

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub before: f32,
    pub after: f32,
    pub examples: usize,
    pub kept: bool,
}

/// A practice request for a new picture: its signature, checklist, and the
/// prompts written for it with their scores.
struct NewCase {
    ask: Ask,
    checklist: Vec<String>,
    tries: Vec<(Written, f32)>,
}

/// A practice edit: its source picture, description, the reference plan
/// (the picture reader's, whose checklist scores every try), and the tries.
struct EditCase {
    request: String,
    source: image::DynamicImage,
    caption: String,
    rubric: EditPlan,
    tries: Vec<(EditPlan, f32)>,
}

/// Scores an edit like the edit job keeps one: the checks, and a little
/// extra when the rest of the picture stayed put.
async fn edit_score(r: &Helper, before: &image::DynamicImage, after: &image::DynamicImage, rubric: &EditPlan) -> f32 {
    let v = prompts::judge_edit(r, before, after, rubric).await.map_or(0.0, |v| v.score);
    let local = rubric.changes["leaving"].as_array().is_some_and(|l| !l.is_empty());
    v + if local && imaging::keep_unchanged(before, before, after).is_some() { 0.1 } else { 0.0 }
}

/// Coaches `m`'s writer. `render` makes a new picture, `edit` runs an edit's
/// steps (None: `m` doesn't edit, so only new pictures are practised).
pub async fn train(state: &Arc<AppState>, m: &MediaModel, render: prompts::Render<'_>, edit: Option<Edit<'_>>, progress: &(dyn Fn(&str, f64) + Sync), cancel: &AtomicBool) -> Result<Report, String> {
    let stopped = || cancel.load(Ordering::Relaxed);
    let stop = || -> Result<Report, String> { Err(crate::download::CANCELLED.into()) };
    let llm = super::installed_file(state, m, "llm").ok_or("This picture model doesn't bring its own language model, so there's no writer to coach.")?;
    if crate::finetune::training() {
        return Err("A model is being taught right now. Coach the prompt writer once that's done.".into());
    }
    if crate::uncensor::running() {
        return Err(crate::uncensor::BUSY.into());
    }
    if !crate::vision::can_read(state, None) {
        return Err("Coaching needs a model that reads pictures, to score the practice pictures. Install one with a picture reader (Qwen3 VL, for example) first.".into());
    }
    let installed = crate::db::InstalledModel {
        model_id: format!("coach-{}", m.id),
        quant: quant_of(&file_name(&llm)),
        path: llm.to_string_lossy().into_owned(),
        size: std::fs::metadata(&llm).map_or(0, |x| x.len()),
        installed_at: 0,
        tps: None,
        prompt_tps: None,
    };
    let arch = crate::finetune::arch_of(state, &installed).unwrap_or_default();
    if !crate::finetune::supported(&arch) || !crate::finetune::conversations_ok(&arch) {
        return Err(format!("{}'s language model ({arch}) can't be trained here yet.", m.name));
    }
    crate::finetune::fits_card(state, &installed)?;
    // The trainer first: Offline, or a download that fails, now stops it
    // here rather than after half an hour of practice pictures.
    progress("Getting the trainer", 0.01);
    crate::finetune::ensure_trainer(state, cancel, 0.01).await?;
    let qwen3 = arch.starts_with("qwen3");
    let name = format!("{}'s language model", m.name);
    let program = prompts::program(&state.paths, prompts::family(m));
    let edits_too = edit.is_some() && m.can("edit");

    // 1. What the untrained writer writes: checklists and three prompts per
    //    request (the first at its steadiest: the score to beat).
    progress("Writing practice prompts", 0.02);
    let w = Helper::start(state, &name, &llm, None).await?;
    let mut news: Vec<NewCase> = Vec::new();
    for (request, look, shape) in prompts::PRACTICE {
        if stopped() {
            w.done(state).await;
            return stop();
        }
        let ask = Ask { request: request.to_string(), look: look.to_string(), shape: shape.to_string(), ..Default::default() };
        let checklist = prompts::checklist(&w, request).await;
        let mut tries = Vec::new();
        for t in [0.2, 0.8, 0.8] {
            if let Ok(x) = prompts::write(&w, &program, &ask, t).await {
                tries.push((x, 0.0));
            }
        }
        news.push(NewCase { ask, checklist, tries });
    }
    w.done(state).await;

    // 2. The practice edits' pictures, described, with a reference plan.
    let mut edits: Vec<EditCase> = Vec::new();
    if edits_too {
        for (i, (scene, _)) in EDITS.iter().enumerate() {
            if stopped() {
                return stop();
            }
            progress(&format!("Making practice picture {} of {}", i + 1, EDITS.len()), 0.05 + 0.05 * i as f64 / EDITS.len() as f64);
            if let Ok(img) = render(scene.to_string(), String::new(), 3000 + i as u64).await {
                edits.push(EditCase { request: EDITS[i].1.to_string(), source: img, caption: String::new(), rubric: EditPlan::default(), tries: Vec::new() });
            }
        }
        progress("Looking at the practice pictures", 0.1);
        let r = prompts::reader(state, Some(m)).await?;
        for e in &mut edits {
            if stopped() {
                break;
            }
            e.caption = prompts::caption(&r, &e.source).await.unwrap_or_default();
            if let Ok(p) = prompts::plan_edit(&r, &e.source, &e.request).await {
                e.rubric = p.clone();
                e.tries.push((p, 0.0));
            }
        }
        r.done(state).await;
        edits.retain(|e| !e.caption.is_empty() && !e.rubric.checklist.is_empty());
        // The writer's own plans from the description.
        progress("Writing practice edit plans", 0.14);
        let w = Helper::start(state, &name, &llm, None).await?;
        for e in &mut edits {
            if stopped() {
                break;
            }
            for t in [0.3, 0.9] {
                if let Ok(p) = prompts::plan_from_words(&w, &e.caption, &e.request, t).await {
                    e.tries.push((p, 0.0));
                }
            }
        }
        w.done(state).await;
    }
    if stopped() {
        return stop();
    }

    // 3. Model 2 makes every try; the reader scores them all.
    let made = score_all(state, m, render, edit, &mut news, &mut edits, progress, cancel, (0.17, 0.6)).await?;
    crate::applog::line("media", &format!("Coaching {}, the untrained writer's tries: {}", m.name, scores_line(&news, &edits)));
    // The score to beat: the writer's steadiest try on each request (its
    // own plan, for an edit; the reader's is the first try there).
    let firsts: Vec<f32> = news.iter().filter_map(|c| c.tries.first().map(|t| t.1)).chain(edits.iter().filter_map(|e| e.tries.get(1).map(|t| t.1))).collect();
    let before = firsts.iter().sum::<f32>() / firsts.len().max(1) as f32;

    // 4. The best try on each request, when good enough, as an example.
    let mut lines = Vec::new();
    for c in &news {
        if let Some((best, s)) = c.tries.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
            if *s >= GOOD_PICTURE {
                lines.push(example(&prompts::writer_messages(&program, &c.ask, true), &json!({ "prompt": best.prompt, "negative": best.negative }).to_string(), qwen3));
            }
        }
    }
    for e in &edits {
        if let Some((best, s)) = e.tries.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
            if *s >= GOOD_EDIT {
                lines.push(example(&prompts::planner_messages(&e.caption, &e.request, true), &best.changes.to_string(), qwen3));
            }
        }
    }
    crate::applog::line("media", &format!("Coaching {}: {made} practice pictures, {} worth learning from, scoring {:.0}% before", m.name, lines.len(), before * 100.0));
    if lines.len() < MIN_EXAMPLES {
        return Err(format!(
            "Only {} of the practice attempts came out well enough to learn from ({MIN_EXAMPLES} are needed), so the writer wasn't trained. A bigger picture model, or a better picture reader, makes more of them good.",
            lines.len()
        ));
    }

    // 5. Training: an adapter on the writer, from its best attempts.
    let folder = dir(&state.paths, &m.id);
    let work = folder.join("training");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    // Its data, the writer's copy and checkpoints (GBs) go however this ends.
    struct Gone(std::path::PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _gone = Gone(work.clone());
    // While it trains, chats don't load a model onto the card beside it.
    crate::finetune::begin(crate::finetune::Job::new(&format!("coach-{}", m.id), &format!("{}'s prompt writer", m.name), &m.name, 3))?;
    struct Ended;
    impl Drop for Ended {
        fn drop(&mut self) {
            crate::finetune::end("done");
        }
    }
    let ended = Ended;
    let data = work.join("data.jsonl");
    std::fs::write(&data, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    progress("Training the prompt writer", 0.62);
    let ready = crate::finetune::prepare(state, &installed, &work, 0.62, cancel).await?;
    let setup = crate::finetune::Setup { conversations: true, epochs: 3, strength: "normal", own_words: true, ctx: TRAIN_CTX, template: None, span: (0.62, 0.8) };
    ready.train(state, &format!("coach-{}", m.id), &data, &setup, cancel).await?;
    drop(ready);
    drop(ended);
    let adapter = work.join("adapter.gguf");
    if !adapter.exists() {
        return Err("The trainer finished without saving the prompt writer's adapter.".into());
    }

    // 6. The trained writer on the same requests, scored the same way.
    progress("Trying the coached prompt writer", 0.8);
    let w = Helper::start_tuned(state, &name, &llm, None, Some(&adapter)).await?;
    for c in &mut news {
        let t = prompts::write(&w, &program, &c.ask, 0.2).await.ok();
        c.tries = t.map(|x| vec![(x, 0.0)]).unwrap_or_default();
    }
    for e in &mut edits {
        let p = prompts::plan_from_words(&w, &e.caption, &e.request, 0.3).await.ok();
        e.tries = p.map(|x| vec![(x, 0.0)]).unwrap_or_default();
    }
    w.done(state).await;
    score_all(state, m, render, edit, &mut news, &mut edits, progress, cancel, (0.82, 0.98)).await?;
    crate::applog::line("media", &format!("Coaching {}, the coached writer's tries: {}", m.name, scores_line(&news, &edits)));
    let all: Vec<f32> = news.iter().map(|c| c.tries.first().map_or(0.0, |t| t.1)).chain(edits.iter().map(|e| e.tries.first().map_or(0.0, |t| t.1))).collect();
    let after = all.iter().sum::<f32>() / all.len().max(1) as f32;
    let kept = after > before + MARGIN;
    crate::applog::line("media", &format!("Coached {}'s prompt writer: {:.0}% → {:.0}%{}", m.name, before * 100.0, after * 100.0, if kept { "; kept" } else { "; not better, so not kept" }));
    if kept {
        let keep = folder.join("adapter.gguf");
        std::fs::rename(&adapter, &keep).map_err(|e| e.to_string())?;
        let c = Coach { at: crate::db::now_ms(), model_id: m.id.clone(), writer: file_name(&llm), adapter: keep, before, after, examples: lines.len() };
        std::fs::write(folder.join("coach.json"), serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&work);
    progress("Done", 1.0);
    Ok(Report { before, after, examples: lines.len(), kept })
}

/// Every try's score in percent, a request at a time, for the log: new
/// pictures, then edits (the reader's own plan first there, before the
/// untrained writer's).
fn scores_line(news: &[NewCase], edits: &[EditCase]) -> String {
    let pct = |tries: Vec<f32>| tries.iter().map(|s| format!("{:.0}", s * 100.0)).collect::<Vec<_>>().join(" ");
    let pictures: Vec<String> = news.iter().map(|c| format!("[{}]", pct(c.tries.iter().map(|t| t.1).collect()))).collect();
    let changes: Vec<String> = edits.iter().map(|e| format!("[{}]", pct(e.tries.iter().map(|t| t.1).collect()))).collect();
    format!("pictures {}; edits {}", pictures.join(" "), if changes.is_empty() { "none".to_string() } else { changes.join(" ") })
}

/// Makes every try (new pictures, then edits) and scores them with the
/// picture reader. Returns how many pictures were made. Every try at one
/// request is made with that request's own seed, before training and after,
/// so the scores differ by the prompt or plan only.
#[allow(clippy::too_many_arguments)]
async fn score_all(
    state: &Arc<AppState>,
    m: &MediaModel,
    render: prompts::Render<'_>,
    edit: Option<Edit<'_>>,
    news: &mut [NewCase],
    edits: &mut [EditCase],
    progress: &(dyn Fn(&str, f64) + Sync),
    cancel: &AtomicBool,
    (from, to): (f64, f64),
) -> Result<usize, String> {
    let total = news.iter().map(|c| c.tries.len()).sum::<usize>() + edits.iter().map(|e| e.tries.len()).sum::<usize>();
    let mut made = 0usize;
    let step = |n: usize| from + (to - from) * 0.8 * n as f64 / total.max(1) as f64;
    let mut new_pics: Vec<Vec<Option<image::DynamicImage>>> = Vec::new();
    for (i, c) in news.iter().enumerate() {
        let mut row = Vec::new();
        for (x, _) in &c.tries {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::download::CANCELLED.into());
            }
            progress(&format!("Making practice picture {} of {total}", made + 1), step(made));
            row.push(render(x.prompt.clone(), x.negative.clone(), 2000 + i as u64).await.ok());
            made += 1;
        }
        new_pics.push(row);
    }
    let mut edit_pics: Vec<Vec<Option<image::DynamicImage>>> = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        let mut row = Vec::new();
        for (p, _) in &e.tries {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::download::CANCELLED.into());
            }
            progress(&format!("Making practice picture {} of {total}", made + 1), step(made));
            let out = match edit {
                Some(run) => run(e.source.clone(), p.steps.clone(), 5000 + i as i64).await.ok(),
                None => None,
            };
            row.push(out);
            made += 1;
        }
        edit_pics.push(row);
    }
    progress("Checking the practice pictures", from + (to - from) * 0.8);
    let r = prompts::reader(state, Some(m)).await?;
    for (c, row) in news.iter_mut().zip(&new_pics) {
        for ((_, score), img) in c.tries.iter_mut().zip(row) {
            *score = match img {
                Some(img) => prompts::judge(&r, img, &c.checklist).await.map_or(0.0, |v| v.score),
                None => 0.0,
            };
        }
    }
    for (e, row) in edits.iter_mut().zip(&edit_pics) {
        for ((_, score), img) in e.tries.iter_mut().zip(row) {
            *score = match img {
                Some(img) => edit_score(&r, &e.source, img, &e.rubric).await,
                None => 0.0,
            };
        }
    }
    r.done(state).await;
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quantization_is_read_from_the_file_name() {
        assert_eq!(quant_of("Qwen3-4B-Q4_K_M.gguf"), "Q4_K_M");
        assert_eq!(quant_of("Qwen3-8B-Q8_0.gguf"), "Q8_0");
        assert_eq!(quant_of("qwen3-4b-instruct-2507-q4_k_m.gguf"), "Q4_K_M");
        assert_eq!(quant_of("strange.gguf"), "Q4_K_M");
    }

    #[test]
    fn an_example_is_a_conversation_ending_in_the_answer() {
        let msgs = prompts::words_messages("A man on a stool, a guitar on the wall.", "have him play the guitar");
        let line = example(&msgs, r#"{"leaving":[],"result":"x","checklist":[]}"#, true);
        let v: Value = serde_json::from_str(&line).unwrap();
        let all = v["messages"].as_array().unwrap();
        assert_eq!(all.len(), msgs.len() + 1);
        assert_eq!(all[0]["role"], "system");
        assert!(all[0]["content"].as_str().unwrap().contains("Read the description of the picture."), "the words-only planner");
        let last = all.last().unwrap();
        assert_eq!(last["role"], "assistant");
        assert!(last["content"].as_str().unwrap().starts_with("<think>\n\n</think>\n\n{"), "Qwen3 answers after an empty thinking block");
        assert!(!example(&msgs, "{}", false).contains("<think>"));
        assert!(all[all.len() - 2]["content"].as_str().unwrap().contains("(The picture: A man on a stool"));
    }

    #[test]
    fn a_training_example_keeps_what_the_writer_is_for_and_the_request() {
        let ask = Ask { request: "a cozy cabin interior with a fireplace on the left".into(), look: "photo".into(), shape: "square".into(), ..Default::default() };
        let full = prompts::messages(&prompts::bundled("flux2"), &ask);
        let short = compact(&full);
        assert_eq!(short.len(), 2, "no worked examples");
        assert_eq!(short[0]["content"], "You write prompts for FLUX.2, a picture model that reads plain, natural sentences with a strong language model. Reply with only JSON.");
        assert_eq!(short[1], *full.last().unwrap(), "the request as the writer gets it");
        let plan = compact(&prompts::words_messages("A man on a stool, a guitar on the wall.", "have him play the guitar"));
        assert_eq!(plan[0]["content"], "You plan a change to a picture for an edit model. Reply with only JSON.");
        assert!(plan[1]["content"].as_str().unwrap().starts_with("The request: have him play the guitar (The picture: A man on a stool"));
        // Well inside the trainer's window with a long answer (about 4.4
        // characters a token in Qwen3's vocabulary).
        let long = format!("{{\"prompt\": \"{}\", \"negative\": \"\"}}", "a warm wooden cabin room ".repeat(36));
        assert!(example(&short, &long, true).len() < TRAIN_CTX as usize * 4, "{}", example(&short, &long, true).len());
    }

    #[test]
    fn a_coached_writer_gets_what_it_was_trained_on() {
        let program = prompts::bundled("flux2");
        let ask = Ask { request: "three red apples in a wooden bowl".into(), look: "photo".into(), ..Default::default() };
        assert_eq!(prompts::writer_messages(&program, &ask, false), prompts::messages(&program, &ask), "an untrained writer gets the whole program");
        assert_eq!(prompts::planner_messages("A man on a stool.", "have him stand", false), prompts::words_messages("A man on a stool.", "have him stand"));
        // A training line, less its answer, is what the coached writer is asked.
        let line: Value = serde_json::from_str(&example(&prompts::writer_messages(&program, &ask, true), "{}", true)).unwrap();
        let asked = line["messages"].as_array().unwrap();
        assert_eq!(asked[..asked.len() - 1], prompts::writer_messages(&program, &ask, true)[..]);
        assert_eq!(prompts::writer_messages(&program, &ask, true), compact(&prompts::messages(&program, &ask)));
        assert_eq!(prompts::planner_messages("A man on a stool.", "have him stand", true), compact(&prompts::words_messages("A man on a stool.", "have him stand")));
    }

    #[test]
    fn every_practice_edit_names_a_scene_and_a_change() {
        assert_eq!(EDITS.len(), 8);
        assert!(EDITS.iter().all(|(scene, change)| scene.starts_with("a photo of") && !change.is_empty()));
        // The kinds that go wrong most: something moved or lifted, something removed, a new look.
        assert!(EDITS.iter().any(|(_, c)| prompts::moved_thing(c).is_some()));
        assert!(EDITS.iter().any(|(_, c)| c.starts_with("remove")));
        assert!(EDITS.iter().any(|(_, c)| c.contains("night")));
    }
}

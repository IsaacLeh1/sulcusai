// SPDX-License-Identifier: AGPL-3.0-only
//! What the prompt writer learns as it's used. Nothing is trained: what's
//! learnt goes into model 1's prompt, the way DSPy builds a program from
//! examples that passed its metric. Model 1 (the writer for new pictures,
//! the planner for edits) puts a request into words for model 2 (the
//! picture model); the reviewer (the picture reader) checks what came out
//! against the request and says, for each thing missing, whether the prompt
//! asked for it; model 1 writes again until it shows, up to `MAX_MAKES`
//! pictures, and the best is kept. From every checked job it keeps:
//! - examples: requests and the prompt or plan that passed. For a new
//!   request the most alike become its worked examples, so it writes the
//!   way that worked for requests like the person's own;
//! - rules about model 2: when a rewrite showed what the try before it
//!   missed, the rule model 1 said that rewrite follows. A rule seen again
//!   counts twice; the most confirmed are shown to model 1;
//! - how first tries score over time, to show it learning.
//!
//! Kept for each picture model, encrypted with the person's key, and never
//! taken from incognito chats.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::prompts::{Demo, Program};
use super::MediaModel;
use crate::AppState;

/// Pictures one request may take while model 1 writes again.
pub const MAX_MAKES: usize = 5;
/// A try this good is finished.
pub const GOOD: f32 = 0.85;
/// Examples kept of each kind, and how many become worked examples.
const EXAMPLES: usize = 40;
const SHOWN: usize = 3;
/// Rules kept, and how many model 1 is shown.
const RULES: usize = 30;
const RULES_SHOWN: usize = 8;
/// First-try scores kept.
const HISTORY: usize = 200;
/// The setting: learn from what's made (off unless turned on).
const ON_KEY: &str = "prompt_writer_learns";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Learned {
    /// New pictures: what the writer was asked and the prompt that passed.
    pub pictures: Vec<Example>,
    /// Edits: the request with a description of the picture, and the plan that passed.
    pub edits: Vec<Example>,
    /// About writing prompts for this model.
    pub rules: Vec<Rule>,
    /// About planning its edits.
    pub edit_rules: Vec<Rule>,
    /// How the first try of each checked job scored, oldest first.
    pub firsts: Vec<f32>,
    pub edit_firsts: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Example {
    /// The person's own words, to find requests like a new one.
    pub request: String,
    /// What model 1 was given.
    pub ask: String,
    /// What passed, as model 1 answers (JSON).
    pub answer: String,
    pub score: f32,
    /// Pictures it took.
    pub makes: u32,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub text: String,
    /// Rewrites following it that showed what was missing.
    pub helped: u32,
    pub at: i64,
}

/// What the Studio shows.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub pictures: usize,
    pub edits: usize,
    pub rules: usize,
    /// Checked jobs it has seen.
    pub jobs: usize,
    /// First tries of new pictures: the first five, and the last ten (None
    /// until there are enough to tell).
    pub first: Option<f32>,
    pub now: Option<f32>,
    pub edit_first: Option<f32>,
    pub edit_now: Option<f32>,
}

fn path(state: &AppState, m: &MediaModel) -> PathBuf {
    let safe: String = m.id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    state.paths.data.join("prompt_learning").join(format!("{safe}.json"))
}

/// What's been learnt for `m` (nothing yet, or locked: empty).
pub fn load(state: &AppState, m: &MediaModel) -> Learned {
    let Ok(stored) = std::fs::read_to_string(path(state, m)) else { return Learned::default() };
    let Ok(c) = state.work_cipher() else { return Learned::default() };
    c.decrypt(&stored).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(state: &AppState, m: &MediaModel, l: &Learned) -> Result<(), String> {
    let p = path(state, m);
    std::fs::create_dir_all(p.parent().unwrap_or(&state.paths.data)).map_err(|e| e.to_string())?;
    let plain = serde_json::to_string(l).map_err(|e| e.to_string())?;
    std::fs::write(&p, state.work_cipher()?.encrypt(&plain)).map_err(|e| e.to_string())
}

/// Forgets everything learnt for `m`.
pub fn forget(state: &AppState, m: &MediaModel) {
    let _ = std::fs::remove_file(path(state, m));
}

/// Learning from what's made is on. Off unless turned on: three held-out
/// tests found first tries no better with it (-2.5, -4.9 and -5.7 points).
pub fn on(state: &AppState) -> bool {
    crate::db::get(&state.db.lock().unwrap(), ON_KEY).unwrap_or(false)
}

pub fn set_on(state: &AppState, on: bool) -> Result<(), String> {
    crate::db::set(&state.db.lock().unwrap(), ON_KEY, &on)
}

/// Turns learning from what's made on or off (the Studio's checkbox).
#[tauri::command(async)]
pub fn media_set_learning(state: crate::AppStateRef, on: bool) -> Result<(), String> {
    set_on(&state, on)
}

/// This job may be learnt from: learning is on and it isn't from an incognito chat.
pub fn may_learn(state: &AppState, chat_id: Option<&str>) -> bool {
    on(state) && !chat_id.is_some_and(|id| crate::db::chat_incognito(&state.db.lock().unwrap(), id))
}

const SMALL: &[&str] = &["the", "and", "with", "for", "that", "this", "from", "into", "onto", "are", "its", "his", "her", "their", "has", "have", "who", "which", "make", "add", "put", "some", "very", "one"];

fn words(s: &str) -> std::collections::BTreeSet<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3 && !SMALL.contains(w)).map(str::to_string).collect()
}

/// How alike two requests (or rules) are: the share of their words in common.
pub fn alike(a: &str, b: &str) -> f32 {
    let (a, b) = (words(a), words(b));
    let all = a.union(&b).count();
    if all == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f32 / all as f32
}

/// The examples most like `request`, best first (none that share nothing).
pub fn picks<'a>(list: &'a [Example], request: &str, n: usize) -> Vec<&'a Example> {
    let mut scored: Vec<(f32, &Example)> = list.iter().map(|e| (alike(&e.request, request), e)).filter(|(s, _)| *s > 0.0).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.at.cmp(&a.1.at)));
    scored.into_iter().take(n).map(|(_, e)| e).collect()
}

/// The rules shown to model 1: the most confirmed, then the newest.
fn shown(rules: &[Rule]) -> Vec<&Rule> {
    let mut list: Vec<&Rule> = rules.iter().collect();
    list.sort_by(|a, b| b.helped.cmp(&a.helped).then(b.at.cmp(&a.at)));
    list.truncate(RULES_SHOWN);
    list
}

/// The rules as a paragraph for model 1's instructions (empty: none yet).
pub fn rules_text(rules: &[Rule]) -> String {
    let list = shown(rules);
    if list.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = list.iter().map(|r| format!("- {}", r.text)).collect();
    format!("\n\nLearned from earlier pictures with this model (each rule made a missing thing show):\n{}", lines.join("\n"))
}

/// The writer's program for one request, with what's been learnt: the rules
/// in its instructions, and the examples most like this request first among
/// its worked examples.
pub fn apply(p: &mut Program, l: &Learned, request: &str) {
    p.instructions.push_str(&rules_text(&l.rules));
    let mine: Vec<Demo> = picks(&l.pictures, request, SHOWN)
        .into_iter()
        .filter_map(|e| {
            let v: Value = serde_json::from_str(&e.answer).ok()?;
            Some(Demo { ask: e.ask.clone(), prompt: v["prompt"].as_str()?.to_string(), negative: v["negative"].as_str().unwrap_or_default().to_string(), score: Some(e.score) })
        })
        .collect();
    if !mine.is_empty() {
        let keep = 4usize.saturating_sub(mine.len());
        let own: Vec<Demo> = p.demos.iter().take(keep).cloned().collect();
        p.demos = mine.into_iter().chain(own).collect();
    }
}

/// The picture of an edit, in words, from its plan (when no reader is at
/// hand to describe it): what was where before the change.
pub fn scene_of(changes: &Value) -> String {
    let parts: Vec<String> = changes["leaving"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let what = c["what"].as_str()?.trim();
            let now = c["where_now"].as_str().unwrap_or_default().trim();
            (!what.is_empty()).then(|| if now.is_empty() { what.to_string() } else { format!("{what} {now}") })
        })
        .collect();
    if parts.is_empty() {
        "as the request describes it".to_string()
    } else {
        parts.join("; ")
    }
}

/// What the edit planner gets besides its instructions: the rules, and the
/// edits most like this request as worked examples.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlannerHelp {
    pub rules: String,
    pub demos: Vec<Value>,
}

pub fn planner_help(l: &Learned, request: &str) -> PlannerHelp {
    let demos = picks(&l.edits, request, 2).into_iter().flat_map(|e| [json!({ "role": "user", "content": e.ask }), json!({ "role": "assistant", "content": e.answer })]).collect();
    PlannerHelp { rules: rules_text(&l.edit_rules), demos }
}

/// A rule from a rewrite that showed what was missing: counted again when
/// it says what one already says, else added (the least confirmed and
/// oldest go when there are too many).
pub fn add_rule(rules: &mut Vec<Rule>, text: &str, now: i64) {
    let text = text.trim().trim_matches('"').trim();
    let n = text.split_whitespace().count();
    if !(4..=40).contains(&n) {
        return;
    }
    if let Some(r) = rules.iter_mut().find(|r| alike(&r.text, text) >= 0.5) {
        r.helped += 1;
        r.at = now;
        // The shorter way of saying it.
        if text.len() < r.text.len() {
            r.text = text.to_string();
        }
        return;
    }
    rules.push(Rule { text: text.to_string(), helped: 1, at: now });
    if rules.len() > RULES {
        let (i, _) = rules.iter().enumerate().min_by(|a, b| a.1.helped.cmp(&b.1.helped).then(a.1.at.cmp(&b.1.at))).unwrap_or((0, &rules[0]));
        rules.remove(i);
    }
}

/// An example that passed: it replaces one for the same request when it did
/// at least as well, and the oldest go when there are too many.
pub fn add_example(list: &mut Vec<Example>, e: Example) {
    if let Some(i) = list.iter().position(|x| alike(&x.request, &e.request) >= 0.9) {
        if e.score >= list[i].score {
            list[i] = e;
        }
        return;
    }
    list.push(e);
    if list.len() > EXAMPLES {
        list.remove(0);
    }
}

pub fn add_first(list: &mut Vec<f32>, score: f32) {
    list.push(score);
    if list.len() > HISTORY {
        list.remove(0);
    }
}

fn mean(xs: &[f32]) -> Option<f32> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f32>() / xs.len() as f32)
}

/// The first five against the last ten, once there are at least eight.
fn trend(xs: &[f32]) -> (Option<f32>, Option<f32>) {
    if xs.len() < 8 {
        return (None, None);
    }
    (mean(&xs[..5]), mean(&xs[xs.len().saturating_sub(10)..]))
}

pub fn summary(l: &Learned) -> Summary {
    let (first, now) = trend(&l.firsts);
    let (edit_first, edit_now) = trend(&l.edit_firsts);
    Summary { pictures: l.pictures.len(), edits: l.edits.len(), rules: l.rules.len() + l.edit_rules.len(), jobs: l.firsts.len() + l.edit_firsts.len(), first, now, edit_first, edit_now }
}

/// One make of a checked job: its score, what it missed, and the rule model
/// 1 said it followed (for a rewrite).
#[derive(Debug, Clone, Default)]
pub struct Try {
    pub score: f32,
    pub missing: Vec<String>,
    pub lesson: Option<String>,
    /// The try it was made from (None: the one before it).
    pub from: Option<usize>,
}

/// Whether to stop writing again: the best is good enough, the pictures
/// are used up, or the last two rewrites did no better than the best before them.
pub fn finished(tries: &[Try]) -> bool {
    let best = tries.iter().map(|t| t.score).fold(0.0f32, f32::max);
    if tries.is_empty() {
        return false;
    }
    if best >= GOOD || tries.len() >= MAX_MAKES {
        return true;
    }
    tries.len() >= 3 && {
        let before = tries[..tries.len() - 2].iter().map(|t| t.score).fold(0.0f32, f32::max);
        tries[tries.len() - 2..].iter().all(|t| t.score <= before)
    }
}

/// The rules a job confirmed: each rewrite that scored higher than the try
/// it was made from and showed something that try missed.
pub fn confirmed(tries: &[Try]) -> Vec<String> {
    tries
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(i, t)| {
            let base = &tries[t.from.unwrap_or(i - 1).min(i - 1)];
            let fixed = t.score > base.score && base.missing.iter().any(|m| !t.missing.contains(m));
            t.lesson.clone().filter(|_| fixed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(request: &str, at: i64) -> Example {
        Example { request: request.into(), ask: format!("What they want: {request}"), answer: json!({ "prompt": format!("a photo of {request}"), "negative": "" }).to_string(), score: 0.9, makes: 1, at }
    }

    #[test]
    fn requests_like_the_new_one_are_its_examples() {
        let list = vec![ex("a red fox in the snow", 1), ex("a birthday cake with candles and the words HAPPY 5TH", 2), ex("a fox sleeping in snow under pine trees", 3)];
        let got: Vec<&str> = picks(&list, "a fox in deep snow", 3).iter().map(|e| e.request.as_str()).collect();
        assert_eq!(got.len(), 2, "nothing in common: not an example");
        assert!(got.iter().all(|r| r.contains("fox")));
        assert!(picks(&list, "", 3).is_empty());
    }

    #[test]
    fn learned_examples_and_rules_go_into_the_program() {
        let mut p = super::super::prompts::bundled("flux2");
        let own = p.demos.clone();
        let mut l = Learned { pictures: vec![ex("a red fox in the snow", 1)], ..Default::default() };
        add_rule(&mut l.rules, "Put quoted sign text in the first sentence of the prompt.", 5);
        apply(&mut p, &l, "a grey fox in snow at dusk");
        assert!(p.instructions.ends_with("- Put quoted sign text in the first sentence of the prompt."));
        assert_eq!(p.demos[0].ask, "What they want: a red fox in the snow");
        assert_eq!(p.demos[0].prompt, "a photo of a red fox in the snow");
        assert_eq!(p.demos.len(), 1 + own.len().min(3));
        // Nothing alike: the program's own examples stay.
        let mut q = super::super::prompts::bundled("flux2");
        apply(&mut q, &l, "a bakery storefront at night");
        assert_eq!(q.demos.len(), own.len());
    }

    #[test]
    fn a_rule_said_again_counts_again() {
        let mut rules = Vec::new();
        add_rule(&mut rules, "Name the number of each object right at the start of the prompt.", 1);
        add_rule(&mut rules, "Name the number of each object at the start of the prompt.", 2);
        add_rule(&mut rules, "Too short.", 3);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].helped, 2);
        assert_eq!(rules[0].text, "Name the number of each object at the start of the prompt.");
        add_rule(&mut rules, "Describe the light last, after every object and its color.", 4);
        assert_eq!(rules.len(), 2);
        assert!(rules_text(&rules).find("Name the number").unwrap() < rules_text(&rules).find("Describe the light").unwrap(), "the most confirmed first");
        for i in 0..40 {
            add_rule(&mut rules, &format!("Mention item{i} before thing{i} and place{i} always."), 10 + i);
        }
        assert_eq!(rules.len(), RULES);
        assert!(rules.iter().any(|r| r.helped == 2), "the confirmed one stays");
    }

    #[test]
    fn a_better_example_for_the_same_request_replaces_the_old_one() {
        let mut list = vec![ex("a red fox in the snow", 1)];
        add_example(&mut list, Example { score: 1.0, ..ex("a red fox in the snow", 2) });
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].at, 2);
        add_example(&mut list, Example { score: 0.5, ..ex("a red fox in the snow", 3) });
        assert_eq!(list[0].at, 2, "a worse one doesn't");
        for i in 0..50 {
            add_example(&mut list, ex(&format!("subject{i} scene{i} place{i}"), 10 + i));
        }
        assert_eq!(list.len(), EXAMPLES);
    }

    #[test]
    fn it_writes_again_until_good_or_no_better() {
        let t = |score: f32| Try { score, ..Default::default() };
        assert!(!finished(&[]));
        assert!(finished(&[t(0.9)]), "good at once");
        assert!(!finished(&[t(0.5)]));
        assert!(!finished(&[t(0.5), t(0.4)]), "one rewrite that didn't help isn't enough to stop");
        assert!(finished(&[t(0.5), t(0.4), t(0.5)]), "two rewrites no better than the best before them");
        assert!(!finished(&[t(0.5), t(0.4), t(0.6)]));
        assert!(finished(&[t(0.5), t(0.6), t(0.7), t(0.75), t(0.8)]), "five pictures at most");
        assert!(finished(&[t(0.5), t(0.9)]));
    }

    #[test]
    fn a_rule_counts_when_its_rewrite_showed_what_was_missing() {
        let t = |score: f32, missing: &[&str], lesson: Option<&str>| Try { score, missing: missing.iter().map(|s| s.to_string()).collect(), lesson: lesson.map(str::to_string), from: None };
        let tries = vec![t(0.5, &["two moons", "a horse"], None), t(0.75, &["two moons"], Some("Say how many of each thing early.")), t(0.75, &["a horse"], Some("Not better: not kept.")), t(1.0, &[], Some("Name the sky's contents in their own sentence."))];
        assert_eq!(confirmed(&tries), vec!["Say how many of each thing early.".to_string(), "Name the sky's contents in their own sentence.".to_string()]);
        // An edit planned again from the best so far is weighed against that one.
        let from = |mut x: Try, i: usize| {
            x.from = Some(i);
            x
        };
        let edits = vec![t(0.45, &["ball overhead", "both hands"], None), t(0.82, &["both hands"], Some("Describe the arms' pose.")), t(0.45, &["ball overhead"], None), from(t(0.6, &["ball overhead"], Some("Worse than the best: not kept.")), 1)];
        assert_eq!(confirmed(&edits), vec!["Describe the arms' pose.".to_string()]);
    }

    #[test]
    fn writing_again_sees_every_try_and_the_reviewers_notes() {
        use super::super::prompts::{bundled, revise_messages, Ask, Round, Written};
        let p = bundled("flux2");
        let ask = Ask { request: "an astronaut riding a horse, two moons in the sky".into(), ..Default::default() };
        let round = |prompt: &str, score: f32, missing: &[&str], notes: &[&str]| Round {
            written: Written { prompt: prompt.into(), negative: String::new() },
            score,
            missing: missing.iter().map(|s| s.to_string()).collect(),
            notes: notes.iter().map(|s| s.to_string()).collect(),
        };
        let rounds = [round("an astronaut on a horse at sunset", 0.5, &["two moons", "a horse"], &[]), round("an astronaut riding a brown horse", 0.75, &["two moons"], &["two moons: the prompt didn't ask for it clearly; the picture shows one sun instead. Say \"two moons\" in the first sentence."])];
        let msgs = revise_messages(&p, &ask, &rounds, false);
        let start = super::super::prompts::messages(&p, &ask).len();
        assert_eq!(msgs.len(), start + 4, "each try and what came of it");
        assert!(msgs[start]["content"].as_str().unwrap().contains("an astronaut on a horse at sunset"));
        let first = msgs[start + 1]["content"].as_str().unwrap();
        assert!(first.contains("50%") && first.contains("two moons; a horse") && !first.contains("reviewer"));
        let last = msgs[start + 3]["content"].as_str().unwrap();
        assert!(last.contains("75%") && last.contains("The reviewer, who saw the picture") && last.contains("Say \"two moons\" in the first sentence."));
        assert!(last.contains("\"lesson\""));
        // A coached writer gets its short messages first.
        assert_eq!(revise_messages(&p, &ask, &rounds, true).len(), 2 + 4);
    }

    #[test]
    fn learned_edits_are_the_planners_worked_examples() {
        let plan = json!({ "leaving": [{ "what": "the red bicycle", "where_now": "leaning on the tree", "left_behind": "the tree trunk" }], "result": "", "checklist": ["no bicycle"] });
        assert_eq!(scene_of(&plan), "the red bicycle leaning on the tree");
        assert_eq!(scene_of(&json!({ "leaving": [], "result": "night" })), "as the request describes it");
        let l = Learned {
            edits: vec![Example { request: "remove the bicycle".into(), ask: "The request: remove the bicycle (The picture: the red bicycle leaning on the tree)".into(), answer: plan.to_string(), score: 1.0, makes: 2, at: 1 }],
            ..Default::default()
        };
        let help = planner_help(&l, "remove the blue bicycle by the fence");
        assert_eq!(help.demos.len(), 2, "a request and its plan");
        assert_eq!(help.demos[0]["role"], "user");
        assert_eq!(help.demos[1]["content"], plan.to_string());
        assert_eq!(help.rules, "", "no rules yet");
        assert!(planner_help(&l, "make it night").demos.is_empty());
    }

    #[test]
    fn progress_shows_once_there_are_enough_jobs() {
        let mut l = Learned::default();
        for s in [0.5, 0.5, 0.6, 0.5, 0.4, 0.7, 0.8] {
            add_first(&mut l.firsts, s);
        }
        assert_eq!(summary(&l).now, None);
        add_first(&mut l.firsts, 0.9);
        let s = summary(&l);
        assert!((s.first.unwrap() - 0.5).abs() < 1e-6);
        assert!((s.now.unwrap() - 0.6125).abs() < 1e-6);
        assert_eq!(s.jobs, 8);
    }
}

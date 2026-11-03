// SPDX-License-Identifier: AGPL-3.0-only
//! Which model Mail, Calendar and the Assistant use: what a model must be
//! able to do for each job, advice on which catalog models can and fit this
//! PC's card, and a short test that the chosen model does the work.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::catalog::{self, Budget, Catalog, ModelSpec, Placement};
use crate::db;

// ---------- the rules ----------

/// What a model needs to do one job. Sized from the baseline tests, where
/// 1.7B models failed the mail and task questions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    pub min_params_b: f64,
    pub needs_tools: bool,
    pub min_ctx: u32,
}

// Sorting a test inbox: Qwen3 1.7B and 4B kept ads and missed a person's
// mail and a security alert; Qwen3.5 9B sorted all of it right.
pub const MAIL_RULES: Rules = Rules { min_params_b: 7.0, needs_tools: true, min_ctx: 16_384 };
pub const CALENDAR_RULES: Rules = Rules { min_params_b: 7.0, needs_tools: true, min_ctx: 16_384 };
pub const ASSISTANT_RULES: Rules = Rules { min_params_b: 4.0, needs_tools: true, min_ctx: 16_384 };

/// Listed sizes are rounded ("4B" models weigh in at 3.96 or 4.02), so a
/// model this close under the minimum still counts.
const SIZE_SLACK_B: f64 = 0.1;

/// The most models advice lists: enough to choose from, not a catalog.
const MAX_ADVICE: usize = 4;

/// Share of the fit-test questions a model must answer well to pass: one
/// slip in three is tolerated, two is not.
const PASS_SHARE: f64 = 0.6;

/// Longest reply a test question waits for.
const TEST_MAX_TOKENS: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Mail,
    Calendar,
    Assistant,
}

impl Feature {
    pub fn rules(self) -> Rules {
        match self {
            Feature::Mail => MAIL_RULES,
            Feature::Calendar => CALENDAR_RULES,
            Feature::Assistant => ASSISTANT_RULES,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Feature::Mail => "Mail",
            Feature::Calendar => "Calendar",
            Feature::Assistant => "the Assistant",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Feature::Mail => "mail",
            Feature::Calendar => "calendar",
            Feature::Assistant => "assistant",
        }
    }
}

/// Why a model can't do a job, in plain words; `Ok` when it can.
pub fn check(spec: &ModelSpec, feature: Feature) -> Result<(), Vec<String>> {
    let rules = feature.rules();
    let what = feature.label();
    let mut why = Vec::new();
    // A model whose size isn't known (an imported file) gets the benefit of
    // the doubt; the fit test is what catches a weak one.
    if spec.params_b > 0.0 && spec.params_b + SIZE_SLACK_B < rules.min_params_b {
        why.push(format!("Too small for {what}: it has {}B parameters and the job needs at least {}B.", spec.params_b, rules.min_params_b));
    }
    if rules.needs_tools && !spec.tools {
        why.push(format!("It can't call tools, which {what} needs."));
    }
    if spec.arch.max_ctx < rules.min_ctx {
        why.push(format!("Its memory of the conversation is too short: {what} needs at least {}K and it holds {}K.", rules.min_ctx / 1024, spec.arch.max_ctx / 1024));
    }
    if why.is_empty() {
        Ok(())
    } else {
        Err(why)
    }
}

// ---------- advice ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Advice {
    pub model_id: String,
    pub name: String,
    pub params_b: f64,
    pub quant: String,
    /// Download size of the version advised.
    pub size: u64,
    pub est_tps: f64,
}

/// Catalog models that meet the job's rules and run on this PC at the
/// window the job needs, smallest first (the cheapest that is good enough),
/// leaving out models already installed. With a graphics card the model must
/// fit on it entirely: a split model is slow to read long mail.
pub fn advice(feature: Feature, budget: &Budget, catalog: &Catalog, installed: &dyn Fn(&str) -> bool) -> Vec<Advice> {
    let rules = feature.rules();
    let mut found: Vec<(&ModelSpec, Advice)> = catalog
        .models
        .iter()
        .filter(|m| m.tags.iter().any(|t| t == "chat") && check(m, feature).is_ok() && !installed(&m.id))
        .filter_map(|m| {
            let ctx = rules.min_ctx.min(m.arch.max_ctx);
            // The highest quality that still runs at the needed window.
            let (v, f) = m
                .variants
                .iter()
                .map(|v| (v, catalog::fit_variant(m, v, ctx, budget)))
                .filter(|(_, f)| f.runnable() && f.disk_ok && (budget.vram == 0 || f.placement == Some(Placement::Gpu)))
                .max_by_key(|(v, _)| v.size)?;
            Some((m, Advice { model_id: m.id.clone(), name: m.name.clone(), params_b: m.params_b, quant: v.quant.clone(), size: v.size, est_tps: f.est_tps }))
        })
        .collect();
    found.sort_by(|a, b| {
        a.0.params_b
            .partial_cmp(&b.0.params_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.0.ratings.overall.cmp(&a.0.ratings.overall))
    });
    found.into_iter().take(MAX_ADVICE).map(|(_, a)| a).collect()
}

// ---------- the fit test ----------

/// One thing a good answer says: any one of these words or figures does.
pub struct Expect(pub &'static [&'static str]);

pub struct Question {
    pub id: &'static str,
    pub prompt: &'static str,
    pub expects: &'static [Expect],
    /// Words that mean the model got it wrong (the other option, a wrong date).
    pub forbids: &'static [&'static str],
}

const MAIL_QUESTIONS: &[Question] = &[
    Question {
        id: "mail-deadline",
        prompt: "Email.\nSubject: Quarterly report\nBody: Hi, please send me the final figures by Friday at 5 pm so I can finish the report.\n\nIn one short sentence: what must the reader do, and by when?",
        expects: &[Expect(&["friday"]), Expect(&["figures", "numbers", "report", "data"])],
        forbids: &[],
    },
    Question {
        id: "mail-newsletter",
        prompt: "Email.\nSubject: This week's top 10 deals\nBody: Save up to 60% on shoes and bags this weekend only! View in browser. Unsubscribe.\n\nIs this a newsletter or a personal message? Answer with one word: newsletter or personal.",
        expects: &[Expect(&["newsletter", "promotion", "marketing", "advert"])],
        forbids: &[],
    },
    Question {
        id: "mail-event",
        prompt: "Email.\nSubject: Lunch\nBody: Hey! Are you free for lunch with Dana on 2026-12-03 at 12:30? I booked Cafe Roma.\n\nWrite the event's date, time and place on one line.",
        expects: &[Expect(&["2026-12-03", "dec 3", "december 3", "3 dec", "12/03", "3rd december", "december 3rd"]), Expect(&["12:30", "12.30", "12h30"]), Expect(&["cafe roma", "café roma"])],
        forbids: &[],
    },
];

const CALENDAR_QUESTIONS: &[Question] = &[
    Question {
        id: "calendar-tomorrow",
        prompt: "Today is Monday 2026-11-02. The user says: \"Put the dentist tomorrow at 3 pm.\" Give the event's date as YYYY-MM-DD and its start time as 24-hour HH:MM, on one line.",
        expects: &[Expect(&["2026-11-03"]), Expect(&["15:00", "15.00", "1500"])],
        forbids: &["2026-11-02", "2026-11-04"],
    },
    Question {
        id: "calendar-gap",
        prompt: "Meetings today: 9:00-10:00, 11:00-12:00, 13:00-14:00. What is the first free time slot long enough for a 45 minute call? Give its start time.",
        expects: &[Expect(&["10:00", "10 am", "10am", "10.00", "10 a.m."])],
        forbids: &[],
    },
    Question {
        id: "calendar-length",
        prompt: "A meeting runs from 13:30 to 15:15. How long is it? Answer in minutes.",
        expects: &[Expect(&["105", "1 hour 45", "1h45", "1 h 45", "1.75 hours", "1 hour and 45"])],
        forbids: &[],
    },
];

const ASSISTANT_QUESTIONS: &[Question] = &[
    Question {
        id: "assistant-urgent",
        prompt: "Task list: write the report (due Friday), buy milk (due today), call Sam (no date). Which task is the most urgent? Answer with just the task.",
        expects: &[Expect(&["milk"])],
        forbids: &["report", "call sam"],
    },
    Question {
        id: "assistant-schedule",
        prompt: "The user says: \"Remind me to water the plants every Sunday at 9.\" Write the schedule in a few words: which day, what time, how often.",
        expects: &[Expect(&["sunday"]), Expect(&["9", "09"]), Expect(&["every", "weekly", "each", "repeat", "recurring"])],
        forbids: &[],
    },
    Question {
        id: "assistant-tool",
        prompt: "You can call these tools: search_files(query), send_email(to, body), read_file(path). The user asks: \"Find my tax notes.\" Which single tool do you call first? Name only the tool.",
        expects: &[Expect(&["search_files"])],
        forbids: &["send_email"],
    },
];

pub fn questions(feature: Feature) -> &'static [Question] {
    match feature {
        Feature::Mail => MAIL_QUESTIONS,
        Feature::Calendar => CALENDAR_QUESTIONS,
        Feature::Assistant => ASSISTANT_QUESTIONS,
    }
}

/// A reply without markdown marks, thinking, case or spacing differences.
fn plain(answer: &str) -> String {
    let mut text = answer.to_string();
    while let (Some(a), Some(b)) = (text.find("<think>"), text.find("</think>")) {
        if b < a {
            break;
        }
        text.replace_range(a..b + "</think>".len(), "");
    }
    // A reply that was cut off while still thinking has no answer.
    if let Some(a) = text.find("<think>") {
        text.truncate(a);
    }
    text.chars().filter(|c| !matches!(c, '*' | '_' | '`')).collect::<String>().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Scored {
    pub id: String,
    pub passed: bool,
    /// What the answer lacked or got wrong, in plain words.
    pub problem: Option<String>,
}

pub fn score(q: &Question, answer: &str) -> Scored {
    let text = plain(answer);
    let problem = if text.is_empty() {
        Some("It gave no answer.".to_string())
    } else if let Some(bad) = q.forbids.iter().find(|f| text.contains(*f)) {
        Some(format!("Its answer mentions \"{bad}\", which is wrong here."))
    } else if let Some(missing) = q.expects.iter().find(|e| !e.0.iter().any(|w| text.contains(w))) {
        Some(format!("Its answer doesn't say \"{}\".", missing.0[0]))
    } else {
        None
    };
    Scored { id: q.id.to_string(), passed: problem.is_none(), problem }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    pub passed: usize,
    pub total: usize,
    pub ok: bool,
}

pub fn verdict(scored: &[Scored]) -> Verdict {
    let passed = scored.iter().filter(|s| s.passed).count();
    let total = scored.len();
    Verdict { passed, total, ok: total > 0 && passed as f64 / total as f64 >= PASS_SHARE }
}

#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub verdict: Verdict,
    pub results: Vec<Scored>,
}

// ---------- the choice, saved ----------

/// The model each feature uses; None is the default model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FeatureModels {
    #[serde(default)]
    pub mail: Option<String>,
    #[serde(default)]
    pub calendar: Option<String>,
    #[serde(default)]
    pub assistant: Option<String>,
}

impl FeatureModels {
    pub fn get(&self, feature: Feature) -> Option<&str> {
        match feature {
            Feature::Mail => &self.mail,
            Feature::Calendar => &self.calendar,
            Feature::Assistant => &self.assistant,
        }
        .as_deref()
    }

    pub fn set(&mut self, feature: Feature, model: Option<String>) {
        match feature {
            Feature::Mail => self.mail = model,
            Feature::Calendar => self.calendar = model,
            Feature::Assistant => self.assistant = model,
        }
    }
}

const CHOICE_KEY: &str = "feature_models";
const TESTS_KEY: &str = "feature_model_tests";

pub fn load(conn: &rusqlite::Connection) -> FeatureModels {
    db::get(conn, CHOICE_KEY).unwrap_or_default()
}

/// The model a feature should run on: the chosen one while it is still
/// installed and still meets the rules, else None (the default model).
pub fn chosen_model(state: &crate::AppState, feature: Feature) -> Option<String> {
    // The database lock is taken one statement at a time: `model_spec` locks it too.
    let id = load(&state.db.lock().unwrap()).get(feature)?.to_string();
    db::installed_model(&state.db.lock().unwrap(), &id)?;
    check(&state.model_spec(&id)?, feature).ok()?;
    Some(id)
}

/// The engine for a feature's work: its chosen model, else what background
/// work uses (the model already loaded, or the default).
pub async fn endpoint(state: &Arc<crate::AppState>, feature: Feature) -> Result<(crate::engine::Endpoint, ModelSpec, crate::engine::Hold), String> {
    let pick = chosen_model(state, feature);
    crate::turns::endpoint_for(state, pick.as_deref()).await
}

type Tests = HashMap<String, Verdict>;

fn test_key(feature: Feature, model_id: &str) -> String {
    format!("{}/{model_id}", feature.key())
}

// ---------- what the picker shows ----------

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub model_id: String,
    pub name: String,
    pub params_b: f64,
    /// Empty when the model meets the rules.
    pub refused: Vec<String>,
    /// The last fit test of this model for this feature.
    pub tested: Option<Verdict>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PickerView {
    pub feature: Feature,
    pub minimum_params_b: f64,
    pub minimum_ctx: u32,
    pub current: Option<String>,
    pub installed: Vec<Candidate>,
    /// Shown when no installed model is allowed.
    pub advice: Vec<Advice>,
}

pub fn picker_view(
    feature: Feature,
    installed: &[ModelSpec],
    current: Option<String>,
    tests: &Tests,
    budget: &Budget,
    catalog: &Catalog,
) -> PickerView {
    let candidates: Vec<Candidate> = installed
        .iter()
        .map(|m| Candidate {
            model_id: m.id.clone(),
            name: m.name.clone(),
            params_b: m.params_b,
            refused: check(m, feature).err().unwrap_or_default(),
            tested: tests.get(&test_key(feature, &m.id)).cloned(),
        })
        .collect();
    let is_installed = |id: &str| installed.iter().any(|m| m.id == id);
    let rules = feature.rules();
    PickerView {
        feature,
        minimum_params_b: rules.min_params_b,
        minimum_ctx: rules.min_ctx,
        current,
        advice: advice(feature, budget, catalog, &is_installed),
        installed: candidates,
    }
}

// ---------- commands (register in lib.rs) ----------

fn installed_specs(state: &crate::AppState) -> Vec<ModelSpec> {
    let ids: Vec<String> = db::installed_models(&state.db.lock().unwrap()).into_iter().map(|m| m.model_id).collect();
    ids.iter().filter_map(|id| state.model_spec(id)).collect()
}

#[tauri::command(async)]
pub fn model_fit_view(state: crate::AppStateRef, feature: Feature) -> PickerView {
    let installed = installed_specs(&state);
    let budget = Budget::from_hardware(&state.hardware.read().unwrap());
    let (current, tests) = {
        let conn = state.db.lock().unwrap();
        (load(&conn).get(feature).map(str::to_string), db::get::<Tests>(&conn, TESTS_KEY).unwrap_or_default())
    };
    picker_view(feature, &installed, current, &tests, &budget, &state.catalog)
}

/// Saves the choice; None goes back to the default model. A model that
/// doesn't meet the job's rules is refused with the reasons.
#[tauri::command(async)]
pub fn set_feature_model(state: crate::AppStateRef, feature: Feature, model_id: Option<String>) -> Result<(), String> {
    let model_id = model_id.filter(|m| !m.is_empty());
    if let Some(id) = &model_id {
        if db::installed_model(&state.db.lock().unwrap(), id).is_none() {
            return Err("That model isn't installed.".into());
        }
        let spec = state.model_spec(id).ok_or("That model isn't in the catalog.")?;
        check(&spec, feature).map_err(|why| why.join(" "))?;
    }
    let conn = state.db.lock().unwrap();
    let mut chosen = load(&conn);
    chosen.set(feature, model_id.clone());
    db::set(&conn, CHOICE_KEY, &chosen)?;
    let summary = match &model_id {
        Some(id) => format!("Chose {id} for {}", feature.label()),
        None => format!("{} goes back to the default model", feature.label()),
    };
    db::log_action(&conn, "models", &summary);
    Ok(())
}

/// Asks the model the feature's sample questions and says whether it does
/// the work. Loads the model, so it can take a while.
#[tauri::command]
pub async fn model_fit_test(state: crate::AppStateRef<'_>, feature: Feature, model_id: String) -> Result<TestReport, String> {
    let state = state.inner().clone();
    let (ep, _, _hold) = crate::turns::endpoint_for(&state, Some(&model_id)).await?;
    let mut results = Vec::new();
    for q in questions(feature) {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": "Answer briefly and exactly as asked." }),
            serde_json::json!({ "role": "user", "content": q.prompt }),
        ];
        // A failed request counts as a failed answer: the model is what is being judged.
        let answer = crate::chat::complete(&ep, messages, serde_json::json!({}), TEST_MAX_TOKENS).await.unwrap_or_default();
        results.push(score(q, &answer));
    }
    let verdict = verdict(&results);
    let conn = state.db.lock().unwrap();
    let mut tests: Tests = db::get(&conn, TESTS_KEY).unwrap_or_default();
    tests.insert(test_key(feature, &model_id), verdict.clone());
    db::set(&conn, TESTS_KEY, &tests)?;
    Ok(TestReport { verdict, results })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(params_b: f64, tools: bool, max_ctx: u32) -> ModelSpec {
        let mut m = Catalog::bundled().models.remove(0);
        m.id = "test-model".into();
        m.params_b = params_b;
        m.tools = tools;
        m.arch.max_ctx = max_ctx;
        m
    }

    fn card(vram_gb: u64) -> Budget {
        let gib = catalog::GIB;
        Budget {
            vram: vram_gb * gib,
            vram_card: vram_gb * gib,
            cards: (vram_gb > 0) as u32,
            ram: 16 * gib,
            disk: 500 * gib,
            gpu_bw_gbs: if vram_gb > 0 { 380.0 } else { 0.0 },
            gpu_bw_card: if vram_gb > 0 { 380.0 } else { 0.0 },
            cpu_bw_gbs: 50.0,
        }
    }

    #[test]
    fn a_big_enough_model_with_tools_passes() {
        for f in [Feature::Mail, Feature::Calendar, Feature::Assistant] {
            assert!(check(&spec(8.0, true, 131_072), f).is_ok());
        }
    }

    #[test]
    fn a_small_model_is_refused_with_its_size() {
        let why = check(&spec(1.7, true, 40_960), Feature::Mail).unwrap_err();
        assert_eq!(why.len(), 1);
        assert!(why[0].contains("1.7B") && why[0].contains("Mail"), "{why:?}");
    }

    #[test]
    fn a_rounded_size_still_counts() {
        assert!(check(&spec(3.96, true, 40_960), Feature::Assistant).is_ok());
        assert!(check(&spec(3.2, true, 40_960), Feature::Assistant).is_err());
        assert!(check(&spec(6.95, true, 40_960), Feature::Calendar).is_ok());
        assert!(check(&spec(3.96, true, 40_960), Feature::Mail).is_err(), "a 4B model sorted mail wrong");
    }

    #[test]
    fn every_reason_is_listed() {
        let why = check(&spec(1.0, false, 8_192), Feature::Assistant).unwrap_err();
        assert_eq!(why.len(), 3);
        assert!(why.iter().any(|w| w.contains("tools")));
        assert!(why.iter().any(|w| w.contains("16K") && w.contains("8K")));
    }

    #[test]
    fn an_unknown_size_is_not_refused_for_size() {
        assert!(check(&spec(0.0, true, 32_768), Feature::Mail).is_ok());
    }

    #[test]
    fn advice_lists_fitting_models_smallest_first() {
        let catalog = Catalog::bundled();
        let list = advice(Feature::Mail, &card(12), &catalog, &|_| false);
        assert!(!list.is_empty() && list.len() <= MAX_ADVICE);
        for a in &list {
            let m = catalog.model(&a.model_id).unwrap();
            assert!(check(m, Feature::Mail).is_ok(), "{}", a.model_id);
            assert!(a.size > 0 && a.params_b >= 3.9);
        }
        assert!(list.windows(2).all(|w| w[0].params_b <= w[1].params_b));
        // A 12 GB card can't hold the very large models.
        assert!(list.iter().all(|a| a.params_b < 40.0));
    }

    #[test]
    fn advice_skips_installed_models_and_those_that_dont_fit() {
        let catalog = Catalog::bundled();
        let all = advice(Feature::Mail, &card(12), &catalog, &|_| false);
        let first = all[0].model_id.clone();
        let rest = advice(Feature::Mail, &card(12), &catalog, &|id| id == first);
        assert!(rest.iter().all(|a| a.model_id != first));
        let tiny = advice(Feature::Mail, &card(4), &catalog, &|_| false);
        assert!(tiny.iter().all(|a| a.size < 4 * catalog::GIB), "{tiny:?}");
        let none = Budget { ram: 0, disk: 0, ..card(0) };
        assert!(advice(Feature::Mail, &none, &catalog, &|_| false).is_empty());
    }

    #[test]
    fn every_feature_has_questions_with_expectations() {
        for f in [Feature::Mail, Feature::Calendar, Feature::Assistant] {
            let qs = questions(f);
            assert!(qs.len() >= 3);
            assert!(qs.iter().all(|q| !q.expects.is_empty() && q.expects.iter().all(|e| !e.0.is_empty())));
        }
    }

    #[test]
    fn a_good_answer_scores_and_a_wrong_one_says_why() {
        let q = &questions(Feature::Calendar)[0];
        assert!(score(q, "**2026-11-03** at 15:00").passed);
        let late = score(q, "2026-11-04 15:00");
        assert!(!late.passed && late.problem.unwrap().contains("2026-11-04"));
        let no_time = score(q, "The date is 2026-11-03.");
        assert!(no_time.problem.unwrap().contains("15:00"));
        assert_eq!(score(q, "  ").problem.as_deref(), Some("It gave no answer."));
    }

    #[test]
    fn thinking_is_ignored_when_scoring() {
        let q = &questions(Feature::Assistant)[0];
        // The reasoning mentions the wrong task; the answer doesn't.
        assert!(score(q, "<think>the report is due Friday but milk is today</think>Buy milk").passed);
        assert!(!score(q, "<think>maybe milk").passed);
    }

    #[test]
    fn the_verdict_tolerates_one_slip_in_three() {
        let s = |passed| Scored { id: String::new(), passed, problem: None };
        assert!(verdict(&[s(true), s(true), s(false)]).ok);
        assert!(!verdict(&[s(true), s(false), s(false)]).ok);
        assert!(!verdict(&[]).ok);
    }

    #[test]
    fn the_choice_defaults_to_the_default_model_and_round_trips() {
        let mut m = FeatureModels::default();
        assert_eq!(m.get(Feature::Mail), None);
        m.set(Feature::Calendar, Some("qwen3-8b".into()));
        let json = serde_json::to_string(&m).unwrap();
        let back: FeatureModels = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get(Feature::Calendar), Some("qwen3-8b"));
        assert_eq!(back.get(Feature::Assistant), None);
        // Saved before a feature existed: still loads.
        let old: FeatureModels = serde_json::from_str("{}").unwrap();
        assert_eq!(old, FeatureModels::default());
    }

    #[test]
    fn the_picker_view_marks_refused_models() {
        let catalog = Catalog::bundled();
        let installed = vec![spec(1.7, true, 40_960), { let mut m = spec(8.0, true, 131_072); m.id = "big".into(); m }];
        let mut tests = Tests::new();
        tests.insert(test_key(Feature::Mail, "big"), Verdict { passed: 3, total: 3, ok: true });
        let v = picker_view(Feature::Mail, &installed, Some("big".into()), &tests, &card(12), &catalog);
        assert_eq!(v.installed.len(), 2);
        assert!(!v.installed[0].refused.is_empty() && v.installed[0].tested.is_none());
        assert!(v.installed[1].refused.is_empty() && v.installed[1].tested.as_ref().unwrap().ok);
        assert!(!v.advice.is_empty());
    }
}

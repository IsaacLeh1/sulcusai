// SPDX-License-Identifier: AGPL-3.0-only
//! Comparing models: the same message to each, one after another (each is
//! loaded in turn; with room on the card the one before stays loaded), with
//! their replies streamed side by side and how long each took.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::chat::Delta;
use crate::AppStateRef;

/// Comparisons running now, by their id, to stop them.
static RUNS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = LazyLock::new(Default::default);

/// Sends `prompt` to each of `models` in turn. Events, each with `run` and
/// `index`: compare:start, compare:delta (content or thinking), compare:done
/// (seconds loading and answering, words a second, or an error).
#[tauri::command]
pub async fn compare_models(app: AppHandle, state: AppStateRef<'_>, run: String, models: Vec<String>, prompt: String) -> Result<(), String> {
    if models.len() < 2 {
        return Err("Pick at least two models to compare.".into());
    }
    if prompt.trim().is_empty() {
        return Err("Type what to ask them first.".into());
    }
    let state = state.inner().clone();
    let cancel = Arc::new(AtomicBool::new(false));
    RUNS.lock().unwrap().insert(run.clone(), cancel.clone());
    state.log("chat", &format!("Compared {} models on one message", models.len()));
    for (index, id) in models.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        app.emit("compare:start", json!({ "run": run, "index": index })).ok();
        let asked = Instant::now();
        let (ep, _, _) = match crate::llm_endpoint(&state, Some(id.clone()), false, |_| {}).await {
            Ok(x) => x,
            Err(e) => {
                app.emit("compare:done", json!({ "run": run, "index": index, "error": e })).ok();
                continue;
            }
        };
        let loading = asked.elapsed().as_secs_f64();
        let started = Instant::now();
        let emitter = app.clone();
        let r = run.clone();
        let messages = vec![json!({ "role": "user", "content": prompt })];
        let done = crate::chat::stream(&ep, messages, None, &cancel, move |d| {
            let delta = match d {
                Delta::Content(s) => json!({ "run": r, "index": index, "content": s }),
                Delta::Thinking(s) => json!({ "run": r, "index": index, "thinking": s }),
            };
            emitter.emit("compare:delta", delta).ok();
        })
        .await;
        let answering = started.elapsed().as_secs_f64();
        match done {
            Ok(f) => {
                let tps = f.tps.or_else(|| f.completion_tokens.map(|n| n as f64 / answering.max(0.01)));
                app.emit("compare:done", json!({ "run": run, "index": index, "loading": loading, "answering": answering, "tps": tps, "tokens": f.completion_tokens })).ok();
            }
            Err(e) => {
                app.emit("compare:done", json!({ "run": run, "index": index, "error": e })).ok();
            }
        }
    }
    RUNS.lock().unwrap().remove(&run);
    Ok(())
}

#[tauri::command(async)]
pub fn compare_stop(run: String) {
    if let Some(c) = RUNS.lock().unwrap().get(&run) {
        c.store(true, Ordering::Relaxed);
    }
}

fn merge_messages(prompt: &str, a: &str, b: &str) -> Vec<serde_json::Value> {
    vec![
        json!({ "role": "system", "content": "You compare two answers to the same question and write one answer from both. Start with the merged answer: what both support, plus anything only one got right. Then a short \"Where they agree\" list and a \"Where they differ\" list, saying which answer said what (Answer 1 or Answer 2) and, when you can tell, which is right and why. Don't invent facts neither gave." }),
        json!({ "role": "user", "content": format!("Question:\n{prompt}\n\nAnswer 1:\n{a}\n\nAnswer 2:\n{b}") }),
    ]
}

/// One answer from two, with where they agree and differ, written on this
/// PC's model (the answers may come from cloud models; they don't go out
/// again).
#[tauri::command]
pub async fn compare_merge(state: AppStateRef<'_>, prompt: String, a: String, b: String) -> Result<String, String> {
    let state: Arc<crate::AppState> = state.inner().clone();
    if a.trim().is_empty() || b.trim().is_empty() {
        return Err("Both models need an answer first.".into());
    }
    let (ep, _spec, _hold) = crate::turns::endpoint_for(&state, None).await?;
    if ep.cloud.is_some() {
        return Err("Merging runs on this PC, and no model on it is ready.".into());
    }
    let merged = crate::chat::complete(&ep, merge_messages(&prompt, &a, &b), json!({ "temperature": 0.2 }), 2000).await?;
    Ok(crate::chat::strip_thinking(&merged).trim().to_string())
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn both_answers_and_the_question_go_to_the_merger() {
        let m = merge_messages("Capital of Australia?", "Canberra.", "Sydney.");
        let asked = m[1]["content"].as_str().unwrap();
        assert!(asked.contains("Answer 1:\nCanberra.") && asked.contains("Answer 2:\nSydney.") && asked.starts_with("Question:\nCapital of Australia?"));
        assert!(m[0]["content"].as_str().unwrap().contains("Where they differ"));
    }
}

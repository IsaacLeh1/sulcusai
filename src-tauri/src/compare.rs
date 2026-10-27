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

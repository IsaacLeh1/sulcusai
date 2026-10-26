// SPDX-License-Identifier: AGPL-3.0-only
//! The graphics card first. When work meant for the card can't go there (no
//! card, its driver, its memory full, its libraries not loaded yet), the
//! person is told why and asked before it runs on the processor and system
//! memory instead: slower, often several times, and hotter. They can say
//! yes once or always, free the card and try again, start SulcusAI again
//! when that's what it takes, or stop. Each time, and the answer, goes in
//! the app log ("card" lines).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;
use tokio::sync::oneshot;

use crate::{download, AppState, AppStateRef};

/// What to do when the card can't take work: "ask" (the default), "always"
/// use the processor, or "never" (the work stops instead).
const SETTING: &str = "processor_fallback";

/// Go on on the processor, or try the card again (it was freed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Go {
    Processor,
    Again,
}

/// The question, as the window shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Ask {
    pub id: String,
    /// The Studio job it's for, when it's one.
    pub job: Option<String>,
    /// What can't go on the card ("Singing the song in your voice").
    pub what: String,
    /// Why not.
    pub why: String,
    /// A chat model holds the card and nothing is using it: unloading it
    /// may make room.
    pub free: bool,
    /// The card's libraries are here but this run loaded the processor's:
    /// starting SulcusAI again uses the card.
    pub restart: bool,
}

static ASKS: LazyLock<Mutex<HashMap<String, (Ask, oneshot::Sender<String>)>>> = LazyLock::new(Default::default);

/// What the person chose for when the card can't take work.
pub fn setting(state: &AppState) -> String {
    crate::db::get::<String>(&state.db.lock().unwrap(), SETTING).filter(|s| matches!(s.as_str(), "ask" | "always" | "never")).unwrap_or_else(|| "ask".into())
}

/// Before `what` runs on the processor because the card can't take it
/// (`why`): asks, unless Settings say always or never. Waits for the answer
/// (Stop on the job ends the wait). `progress` shows the job waiting.
pub async fn processor_ok(state: &Arc<AppState>, job: Option<&str>, what: &str, why: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Go, String> {
    crate::applog::line("card", &format!("{what} can't use the graphics card: {why}"));
    match setting(state).as_str() {
        "always" => {
            crate::applog::line("card", &format!("{what}: on the processor (Settings allow it without asking)"));
            return Ok(Go::Processor);
        }
        "never" => {
            return Err(format!("{what} can't use the graphics card right now ({why}), and Settings say not to use the processor instead. That's in Settings › This PC."));
        }
        _ => {}
    }
    // No window to ask in (tests): as before, the processor.
    let Some(app) = state.app.get().cloned() else { return Ok(Go::Processor) };
    let free = state.generations.lock().unwrap().is_empty() && state.engine.lock().await.status().model_id.is_some();
    let restart = crate::natural::loaded_for_processor() && crate::natural::card_runtime_dll(&state.paths, &state.catalog.voices).is_some();
    let ask = Ask { id: uuid::Uuid::new_v4().simple().to_string(), job: job.map(str::to_string), what: what.to_string(), why: why.to_string(), free, restart };
    let (tx, mut rx) = oneshot::channel();
    ASKS.lock().unwrap().insert(ask.id.clone(), (ask.clone(), tx));
    app.emit("card:ask", &ask).ok();
    progress("Waiting for your answer: the graphics card can't do this right now", 0.0);
    let answer = loop {
        tokio::select! {
            a = &mut rx => break a.unwrap_or_else(|_| "stop".into()),
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if cancel.load(Ordering::Relaxed) {
                    break "stop".into();
                }
            }
        }
    };
    ASKS.lock().unwrap().remove(&ask.id);
    app.emit("card:ask_done", serde_json::json!({ "id": ask.id })).ok();
    crate::applog::line("card", &format!("{what}: asked about the processor, the answer was {answer}"));
    match answer.as_str() {
        "processor" => Ok(Go::Processor),
        "always" => {
            crate::db::set(&state.db.lock().unwrap(), SETTING, &"always")?;
            Ok(Go::Processor)
        }
        "free" => {
            let mut e = state.engine.lock().await;
            if e.status().model_id.is_some() && state.generations.lock().unwrap().is_empty() {
                e.stop().await;
                app.emit("engine:unloaded", serde_json::json!({})).ok();
                crate::applog::line("card", "Unloaded the chat model to make room on the graphics card");
            }
            Ok(Go::Again)
        }
        "restart" => {
            crate::applog::line("card", "Starting SulcusAI again so the graphics card's libraries load");
            app.restart();
        }
        _ => Err(download::CANCELLED.into()),
    }
}

/// How work on a blocking thread asks when the card fails partway: yes to
/// the processor, or stop.
pub type Hook = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

thread_local! {
    static ASK_HERE: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
}

/// The hook for the job running in this task (`processor_ok`, waited for):
/// None outside a job.
pub fn hook_for_job(state: &Arc<AppState>, job: Option<String>) -> Option<Hook> {
    let cancel = crate::vision::JOB_STOP.try_with(|c| c.clone()).ok()?;
    let handle = tokio::runtime::Handle::current();
    let state = state.clone();
    Some(Arc::new(move |what: &str, why: &str| handle.block_on(processor_ok(&state, job.as_deref(), what, why, &cancel, &|_, _| {})).is_ok()))
}

/// Runs `f` (on this thread) with `hook` as how to ask.
pub fn asking<T>(hook: Option<Hook>, f: impl FnOnce() -> T) -> T {
    let before = ASK_HERE.with(|h| h.replace(hook));
    let out = f();
    ASK_HERE.with(|h| *h.borrow_mut() = before);
    out
}

/// The card failed partway through `what` (`why`): may it go on on the
/// processor? Asked when the work runs for a job (`asking`), else yes.
pub fn go_on_on_processor(what: &str, why: &str) -> bool {
    match ASK_HERE.with(|h| h.borrow().clone()) {
        Some(ask) => ask(what, why),
        None => true,
    }
}

/// The questions waiting (a window opened after one was asked shows it).
#[tauri::command]
pub fn card_asks() -> Vec<Ask> {
    ASKS.lock().unwrap().values().map(|(a, _)| a.clone()).collect()
}

/// The answer to a question: "processor", "always", "free", "restart" or "stop".
#[tauri::command]
pub fn card_ask_answer(id: String, answer: String) -> Result<(), String> {
    match ASKS.lock().unwrap().remove(&id) {
        Some((_, tx)) => {
            tx.send(answer).ok();
            Ok(())
        }
        None => Err("That question is no longer waiting.".into()),
    }
}

/// What happens when the card can't take work: "ask", "always" or "never".
#[tauri::command]
pub fn processor_fallback(state: AppStateRef) -> String {
    setting(&state)
}

#[tauri::command]
pub fn set_processor_fallback(state: AppStateRef, value: String) -> Result<(), String> {
    if !matches!(value.as_str(), "ask" | "always" | "never") {
        return Err("Unknown choice.".into());
    }
    crate::applog::line("card", &format!("When the graphics card can't take work: {value}"));
    crate::db::set(&state.db.lock().unwrap(), SETTING, &value)
}

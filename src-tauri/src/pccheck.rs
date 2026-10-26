// SPDX-License-Identifier: AGPL-3.0-only
//! Check this PC: tries each feature in turn (the graphics card and space,
//! chat, pictures, songs, narration, the natural voices, the person's own
//! voice, speech recognition, the documents library) and says which works,
//! how long it took, and why one doesn't. The plan is shown first and each
//! step is ticked off as it finishes ("pccheck:step" events), so it never
//! looks stuck. Nothing big is downloaded: a feature whose model isn't here
//! is skipped and says so. What it makes is deleted again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use serde_json::json;
use tauri::Emitter;

use crate::{AppState, AppStateRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Waiting,
    Running,
    Ok,
    /// Works, with something worth knowing.
    Warn,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    pub status: Status,
    /// What it found, or why it failed or was skipped.
    pub detail: String,
    /// What it's doing now, while it runs.
    pub doing: String,
    pub ms: u64,
}

/// The plan, in order.
const PLAN: &[(&str, &str)] = &[
    ("pc", "This PC: graphics card, memory and space"),
    ("chat", "Chat: the default model answers"),
    ("pictures", "Pictures: a small test picture"),
    ("songs", "Songs: a short test clip"),
    ("narration", "Narration: Windows' voice reads"),
    ("natural", "Natural voices"),
    ("own_voice", "Your voice reads"),
    ("speech", "Speech recognition hears it"),
    ("library", "Documents library: finding passages"),
];

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub running: bool,
    pub started_at: Option<i64>,
    pub took_ms: u64,
    pub steps: Vec<Step>,
}

static REPORT: Mutex<Option<Report>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);

fn fresh() -> Report {
    Report {
        running: true,
        started_at: Some(crate::db::now_ms()),
        took_ms: 0,
        steps: PLAN.iter().map(|(id, title)| Step { id, title, status: Status::Waiting, detail: String::new(), doing: String::new(), ms: 0 }).collect(),
    }
}

fn update(state: &AppState, f: impl FnOnce(&mut Report)) {
    let snapshot = {
        let mut slot = REPORT.lock().unwrap();
        let r = slot.get_or_insert_with(fresh);
        f(r);
        r.clone()
    };
    if let Some(app) = state.app.get() {
        let _ = app.emit("pccheck:step", json!(snapshot));
    }
}

fn set_step(state: &AppState, id: &str, f: impl FnOnce(&mut Step)) {
    update(state, |r| {
        if let Some(s) = r.steps.iter_mut().find(|s| s.id == id) {
            f(s);
        }
    });
}

/// One step's outcome.
type Outcome = (Status, String);

fn gib(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

fn check_pc(state: &AppState) -> Outcome {
    let hw = state.hardware.read().unwrap().clone();
    let mut notes = Vec::new();
    let mut status = Status::Ok;
    match hw.best_gpu() {
        Some(g) => notes.push(format!("{} with {} of graphics memory", g.name, gib(g.vram_bytes))),
        None => {
            status = Status::Warn;
            notes.push("No graphics card SulcusAI can use: models run on the processor (slower).".into());
        }
    }
    for b in &hw.broken_cards {
        status = Status::Warn;
        notes.push(format!("{} isn't working in Windows (its driver may be missing or stopped): update or reinstall its driver.", b.name));
    }
    notes.push(format!("{} of memory, {} free", gib(hw.ram_total), gib(hw.ram_available)));
    if hw.disk_free < 10 * 1_073_741_824 {
        status = Status::Warn;
        notes.push(format!("Only {} free where models are kept ({}): models and downloads may not fit.", gib(hw.disk_free), hw.models_drive));
    } else {
        notes.push(format!("{} free for models on {}", gib(hw.disk_free), hw.models_drive));
    }
    if hw.on_battery == Some(true) {
        notes.push("On battery: plugging in makes everything faster.".into());
    }
    (status, notes.join(". "))
}

async fn check_chat(state: &Arc<AppState>, doing: &(dyn Fn(&str) + Sync)) -> Outcome {
    if state.settings().default_model.is_none() {
        return (Status::Skipped, "No chat model is set up yet: install one in Models.".into());
    }
    doing("Loading the model");
    let started = Instant::now();
    let (ep, spec, _hold) = match crate::background_endpoint(state).await {
        Ok(e) => e,
        Err(e) => return (Status::Failed, format!("The model didn't start: {e}")),
    };
    let loaded = started.elapsed().as_secs_f64();
    doing("Asking it a question");
    let asked = Instant::now();
    let msgs = vec![json!({"role": "user", "content": "What is 12 plus 30? Reply with just the number."})];
    match crate::chat::complete(&ep, msgs, json!({"temperature": 0}), 20).await {
        Ok(a) if a.contains("42") => (Status::Ok, format!("{} answered correctly in {:.1} s (ready in {loaded:.0} s)", spec.name, asked.elapsed().as_secs_f64())),
        Ok(a) => (Status::Warn, format!("{} answered, but not correctly (“{}”)", spec.name, a.chars().take(40).collect::<String>())),
        Err(e) => (Status::Failed, format!("{} didn't answer: {e}", spec.name)),
    }
}

/// A media job run and its result deleted again.
async fn try_media(state: &Arc<AppState>, req: crate::media::Request, doing: &(dyn Fn(&str) + Sync)) -> Result<(f64, String), String> {
    let id = crate::media::store::new_id();
    let started = Instant::now();
    let st = state.clone();
    let job = tokio::spawn({
        let id = id.clone();
        async move { crate::media::run(&st, id, req).await }
    });
    tokio::pin!(job);
    let items = loop {
        tokio::select! {
            r = &mut job => break r.map_err(|e| e.to_string())??,
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if STOP.load(Ordering::Relaxed) {
                    crate::media::cancel(&id);
                }
                if let Some(j) = crate::media::list_jobs().into_iter().find(|j| j.id == id) {
                    doing(&if j.fraction > 0.0 { format!("{} ({:.0}%)", j.stage, j.fraction * 100.0) } else { j.stage.clone() });
                }
            }
        }
    };
    let took = started.elapsed().as_secs_f64();
    let made = items.first().map(|i| i.model_id.clone().unwrap_or_default()).unwrap_or_default();
    let conn = state.db.lock().unwrap();
    for i in &items {
        let _ = crate::media::store::delete(&conn, &state.paths, &i.id);
    }
    Ok((took, made))
}

async fn check_pictures(state: &Arc<AppState>, doing: &(dyn Fn(&str) + Sync)) -> Outcome {
    if !crate::features::is_on(&state.db.lock().unwrap(), crate::features::Feature::Images) {
        return (Status::Skipped, "Pictures are turned off in Features.".into());
    }
    if !crate::media::has_kind(state, crate::media::Kind::Image) {
        return (Status::Skipped, "No picture model is installed.".into());
    }
    let req = crate::media::Request { op: "generate".into(), prompt: "a red apple on a white table".into(), width: Some(384), height: Some(384), count: Some(1), write_prompt: Some(false), check: Some(false), ..Default::default() };
    match try_media(state, req, doing).await {
        Ok((s, m)) => (Status::Ok, format!("Made a picture with {m} in {s:.0} s")),
        Err(e) if e == crate::download::CANCELLED => (Status::Skipped, "Stopped.".into()),
        Err(e) => (Status::Failed, e),
    }
}

async fn check_songs(state: &Arc<AppState>, doing: &(dyn Fn(&str) + Sync)) -> Outcome {
    if !crate::features::is_on(&state.db.lock().unwrap(), crate::features::Feature::Music) {
        return (Status::Skipped, "Music is turned off in Features.".into());
    }
    if !crate::media::has_kind(state, crate::media::Kind::Music) {
        return (Status::Skipped, "No music model is installed.".into());
    }
    let req = crate::media::Request { op: "music".into(), prompt: "calm acoustic guitar".into(), seconds: Some(10.0), instrumental: Some(true), ..Default::default() };
    match try_media(state, req, doing).await {
        Ok((s, m)) => (Status::Ok, format!("Made a 10-second clip with {m} in {s:.0} s")),
        Err(e) if e == crate::download::CANCELLED => (Status::Skipped, "Stopped.".into()),
        Err(e) => (Status::Failed, e),
    }
}

/// Speech that has sound in it, at its rate.
fn heard_sound(s: &crate::tts::Speech) -> bool {
    s.samples.len() > s.rate as usize / 2 && s.samples.iter().any(|x| x.unsigned_abs() > 300)
}

/// Windows' voice reading; its sound is kept for speech recognition.
async fn check_narration() -> (Outcome, Option<crate::tts::Speech>) {
    let read = tokio::task::spawn_blocking(|| crate::tts::synthesize("The quick brown fox checks this computer.", Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0)).await;
    match read {
        Ok(Ok(s)) if heard_sound(&s) => ((Status::Ok, format!("Read {:.1} s of speech", s.samples.len() as f64 / s.rate as f64)), Some(s)),
        Ok(Ok(_)) => ((Status::Failed, "Windows' voice made only silence. Check that a Windows voice is installed (Settings › Time and language › Speech).".into()), None),
        Ok(Err(e)) => ((Status::Failed, e), None),
        Err(e) => ((Status::Failed, e.to_string()), None),
    }
}

async fn check_natural() -> Outcome {
    if crate::natural::installed().is_none() {
        return (Status::Skipped, "The natural voices aren't installed (Models › Speech).".into());
    }
    let read = tokio::task::spawn_blocking(|| crate::tts::synthesize("Natural voices are working.", Some("supertonic:F1"), Some("en"), 1.0)).await;
    match read {
        Ok(Ok(s)) if heard_sound(&s) => (Status::Ok, "A natural voice read a sentence".into()),
        Ok(Ok(_)) => (Status::Failed, "The natural voice made only silence.".into()),
        Ok(Err(e)) => (Status::Failed, e),
        Err(e) => (Status::Failed, e.to_string()),
    }
}

async fn check_own_voice(state: &Arc<AppState>, doing: &(dyn Fn(&str) + Sync)) -> Outcome {
    let Ok(c) = state.work_cipher() else { return (Status::Skipped, "SulcusAI is locked.".into()) };
    let voices = crate::media::own_voice::list(&state.db.lock().unwrap(), &c);
    let Some(v) = voices.iter().find(|v| v.can_speak()) else {
        return (Status::Skipped, if voices.is_empty() { "No voice made yet (Studio › Your voice).".into() } else { "No voice has enough recorded to read yet.".into() });
    };
    if !crate::media::own_voice::ready_here(state) {
        return (Status::Skipped, "The voice engine isn't downloaded yet: it downloads the first time you hear your voice.".into());
    }
    doing(&format!("Reading in {}", v.name));
    let never = AtomicBool::new(false);
    let started = Instant::now();
    match crate::media::own_voice::speak(state, &v.id, "This is my voice, checking that everything works.", Some("en"), &never, &|stage, _| doing(stage)).await {
        Ok((samples, rate)) => (Status::Ok, format!("{} read {:.1} s of speech in {:.0} s", v.name, samples.len() as f64 / rate as f64, started.elapsed().as_secs_f64())),
        Err(e) => (Status::Failed, e),
    }
}

async fn check_speech(state: &Arc<AppState>, said: Option<crate::tts::Speech>, doing: &(dyn Fn(&str) + Sync)) -> Outcome {
    if crate::speech::installed(&state.db.lock().unwrap()).is_empty() {
        return (Status::Skipped, "No speech recognition model is installed (Models › Speech).".into());
    }
    let Some(s) = said else { return (Status::Skipped, "Nothing was read aloud to hear.".into()) };
    doing("Starting speech recognition");
    let _hold = crate::speech::hold(state);
    let ep = match crate::speech::endpoint(state, crate::speech::Use::Accurate).await {
        Ok(e) => e,
        Err(e) => return (Status::Failed, format!("It didn't start: {e}")),
    };
    doing("Listening");
    let f: Vec<f32> = s.samples.iter().map(|x| *x as f32 / 32768.0).collect();
    match crate::speech::transcribe(&ep, &crate::audio::resample(&f, s.rate, crate::audio::RATE), &crate::speech::Options { language: Some("en".into()), ..Default::default() }).await {
        Ok(t) if t.text.to_lowercase().contains("fox") => (Status::Ok, format!("Heard “{}”", t.text.trim())),
        Ok(t) => (Status::Warn, format!("It heard something else: “{}”", t.text.trim())),
        Err(e) => (Status::Failed, e),
    }
}

async fn check_library(state: &Arc<AppState>) -> Outcome {
    if !crate::doclib::has_documents(state) {
        return (Status::Skipped, "Nothing in the documents library yet.".into());
    }
    match crate::doclib::search(state, "what is this about", 1).await {
        Ok(h) if !h.is_empty() => (Status::Ok, if crate::doclib::model_ready(state) { "Found passages by meaning and words".into() } else { "Found passages by their words (its model isn't downloaded yet)".into() }),
        Ok(_) => (Status::Warn, "The library answered, but found nothing.".into()),
        Err(e) => (Status::Failed, e),
    }
}

async fn run_all(state: Arc<AppState>) {
    let started = Instant::now();
    let mut said = None;
    for (id, _) in PLAN {
        if STOP.load(Ordering::Relaxed) {
            set_step(&state, id, |s| {
                s.status = Status::Skipped;
                s.detail = "Stopped.".into();
            });
            continue;
        }
        set_step(&state, id, |s| s.status = Status::Running);
        let t0 = Instant::now();
        let st = state.clone();
        let step_id: &'static str = id;
        let doing = move |what: &str| {
            let what = what.to_string();
            set_step(&st, step_id, |s| s.doing = what);
        };
        let outcome = match *id {
            "pc" => check_pc(&state),
            "chat" => check_chat(&state, &doing).await,
            "pictures" => check_pictures(&state, &doing).await,
            "songs" => check_songs(&state, &doing).await,
            "narration" => {
                let (o, s) = check_narration().await;
                said = s;
                o
            }
            "natural" => check_natural().await,
            "own_voice" => check_own_voice(&state, &doing).await,
            "speech" => check_speech(&state, said.take(), &doing).await,
            "library" => check_library(&state).await,
            _ => (Status::Skipped, String::new()),
        };
        let ms = t0.elapsed().as_millis() as u64;
        set_step(&state, id, |s| {
            s.status = outcome.0;
            s.detail = outcome.1;
            s.doing.clear();
            s.ms = ms;
        });
    }
    let took = started.elapsed().as_millis() as u64;
    update(&state, |r| {
        r.running = false;
        r.took_ms = took;
    });
    let report = REPORT.lock().unwrap().clone().unwrap_or_default();
    let failed: Vec<&str> = report.steps.iter().filter(|s| s.status == Status::Failed).map(|s| s.id).collect();
    crate::applog::line("pccheck", &format!("Checked this PC in {:.0} s: {}", took as f64 / 1000.0, if failed.is_empty() { "nothing failed".to_string() } else { format!("failed: {}", failed.join(", ")) }));
    for s in report.steps.iter().filter(|s| matches!(s.status, Status::Failed | Status::Warn)) {
        crate::applog::line("pccheck", &format!("{}: {}", s.id, s.detail));
    }
}

/// Starts checking (or shows the check already running).
#[tauri::command]
pub fn pc_check_start(state: AppStateRef<'_>) -> Report {
    {
        let slot = REPORT.lock().unwrap();
        if let Some(r) = slot.as_ref().filter(|r| r.running) {
            return r.clone();
        }
    }
    STOP.store(false, Ordering::Relaxed);
    *REPORT.lock().unwrap() = Some(fresh());
    tauri::async_runtime::spawn(run_all(state.inner().clone()));
    REPORT.lock().unwrap().clone().unwrap_or_default()
}

#[tauri::command]
pub fn pc_check_stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// The last check (or the one running).
#[tauri::command]
pub fn pc_check_report() -> Option<Report> {
    REPORT.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_starts_waiting() {
        let r = fresh();
        assert!(r.running);
        assert_eq!(r.steps.len(), PLAN.len());
        assert!(r.steps.iter().all(|s| s.status == Status::Waiting));
        assert_eq!(gib(1_073_741_824 * 3 / 2), "1.5 GB");
    }
}

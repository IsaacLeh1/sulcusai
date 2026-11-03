// SPDX-License-Identifier: AGPL-3.0-only
//! Waking the assistant by saying a phrase ("Hey Sulcus"). While it's on,
//! the microphone is listened to on this PC: short bursts of speech go to
//! the speech model, and the phrase brings the window up with a spoken
//! chat. Nothing heard is kept or sent anywhere.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::audio::Capture;
use crate::vad::{self, Phrase, Phraser};
use crate::{db, speech, AppState, AppStateRef};

const KEY: &str = "wake_word";
/// Speech longer than this isn't someone saying the phrase.
const LONGEST_SECS: f64 = 3.5;
const SHORTEST_SECS: f64 = 0.35;
/// After waking, the phrase is ignored this long (the chat that opens is listening).
const REST: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct WakeSettings {
    pub on: bool,
    pub phrase: String,
}

impl Default for WakeSettings {
    fn default() -> Self {
        WakeSettings { on: false, phrase: "Hey Sulcus".into() }
    }
}

fn settings(state: &AppState) -> WakeSettings {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

pub fn running() -> bool {
    RUNNING.lock().unwrap().as_ref().is_some_and(|stop| !stop.load(Ordering::Relaxed))
}
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// Letters and digits only, lower case, words joined.
fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(row[j + 1] + 1);
            prev = here;
        }
    }
    row[b.len()]
}

/// Whether the phrase was said: somewhere in what was heard, spelled close
/// enough (the speech model writes a name it doesn't know as it sounds,
/// "Hey Sulkus").
pub fn said(heard: &str, phrase: &str) -> bool {
    let want: Vec<char> = letters(phrase).chars().collect();
    let got: Vec<char> = letters(heard).chars().collect();
    if want.is_empty() || got.is_empty() {
        return false;
    }
    let allowed = (want.len() / 4).max(1);
    let lo = want.len().saturating_sub(2).max(1);
    let hi = want.len() + 2;
    (lo..=hi.min(got.len())).any(|len| got.windows(len).any(|w| distance(w, &want) <= allowed))
}

pub(crate) fn stop() {
    if let Some(old) = RUNNING.lock().unwrap().take() {
        old.store(true, Ordering::Relaxed);
    }
}

/// Starts listening for the phrase when that's on (and stops the listener
/// that ran before).
pub fn start(app: AppHandle, state: Arc<AppState>) {
    stop();
    *PROBLEM.lock().unwrap() = None;
    let s = settings(&state);
    if !s.on || letters(&s.phrase).is_empty() {
        return;
    }
    let flag = Arc::new(AtomicBool::new(false));
    *RUNNING.lock().unwrap() = Some(flag.clone());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = listen(&app, &state, &s.phrase, &flag).await {
            state.log("voice", &format!("Listening for the wake word stopped: {e}"));
            *PROBLEM.lock().unwrap() = Some(e);
        }
    });
}

async fn listen(app: &AppHandle, state: &Arc<AppState>, phrase: &str, stop: &AtomicBool) -> Result<(), String> {
    let _hold = speech::hold(state);
    let ep = speech::endpoint(state, speech::Use::Live).await?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let capture = Capture::start(crate::voice::mic(state), tx)?;
    state.log("voice", "Listening for the wake word");
    let mut phraser = Phraser::new(vad::Config { end_ms: 450, ..Default::default() }, LONGEST_SECS + 1.0);
    let mut woke = Instant::now() - REST;
    let opts = speech::Options { prompt: Some(phrase.to_string()), ..Default::default() };
    while !stop.load(Ordering::Relaxed) {
        let chunk = match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(_) => continue,
        };
        for p in phraser.push(&chunk) {
            let Phrase::Done(u) = p else { continue };
            let secs = u.samples.len() as f64 / crate::audio::RATE as f64;
            // Busy talking already (voice chat, dictation, a meeting): the phrase isn't for waking.
            if !(SHORTEST_SECS..=LONGEST_SECS).contains(&secs) || woke.elapsed() < REST || crate::voice::listening_now() || crate::media::live_voice::running() {
                continue;
            }
            let Ok(t) = speech::transcribe(&ep, &u.samples, &opts).await else { continue };
            if said(&t.text, phrase) {
                woke = Instant::now();
                wake(app, state);
            }
        }
    }
    let device_error = capture.error();
    drop(capture);
    device_error.map_or(Ok(()), Err)
}

fn wake(app: &AppHandle, state: &AppState) {
    state.log("voice", "Heard the wake word");
    crate::quick::show_main(app);
    app.emit("wake:heard", json!({})).ok();
}

#[derive(Serialize)]
pub struct WakeView {
    #[serde(flatten)]
    settings: WakeSettings,
    listening: bool,
    problem: Option<String>,
}

fn view(state: &AppState) -> WakeView {
    WakeView { settings: settings(state), listening: RUNNING.lock().unwrap().is_some(), problem: PROBLEM.lock().unwrap().clone() }
}

#[tauri::command(async)]
pub fn wake_word(state: AppStateRef) -> WakeView {
    view(&state)
}

#[tauri::command(async)]
pub fn set_wake_word(app: AppHandle, state: AppStateRef, on: bool, phrase: String) -> Result<WakeView, String> {
    let phrase = phrase.trim().chars().take(40).collect::<String>();
    if on {
        crate::features::require(&state, crate::features::Feature::VoiceChat)?;
        if letters(&phrase).chars().count() < 4 {
            return Err("Use a phrase of a few words, like “Hey Sulcus”: a short one wakes it by mistake.".into());
        }
    }
    db::set(&state.db.lock().unwrap(), KEY, &WakeSettings { on, phrase })?;
    start(app, state.inner().clone());
    Ok(view(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrase_is_heard_however_it_is_spelled() {
        for heard in ["Hey Sulcus.", "hey sulcus", "Hey, Sulkus!", "Hey Sulcis", "Okay. Hey Sulcus, what's on today?", "hey sul cus"] {
            assert!(said(heard, "Hey Sulcus"), "{heard}");
        }
        for heard in ["Hey Siri", "Hello", "They sulked us", "Hey Suzy", "", "Thank you."] {
            assert!(!said(heard, "Hey Sulcus"), "{heard}");
        }
        assert!(said("computer", "Computer"));
        assert_eq!(distance(&['a', 'b'], &['a', 'c', 'b']), 1);
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Dictation (speech typed into a text box) and voice mode (a spoken
//! conversation): the microphone, speech recognition, and spoken replies.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::audio::{Capture, Source};
use crate::speech::{self, SpeechEndpoint};
use crate::tts;
use crate::vad::{self, Phrase, Phraser};
use crate::{AppState, AppStateRef};

/// Longest single dictation.
const MAX_DICTATION: Duration = Duration::from_secs(15 * 60);

fn sessions() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static S: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn begin(key: &str) -> Result<Arc<AtomicBool>, String> {
    let mut s = sessions().lock().unwrap();
    if s.contains_key(key) {
        return Err("Already listening.".into());
    }
    let flag = Arc::new(AtomicBool::new(false));
    s.insert(key.to_string(), flag.clone());
    Ok(flag)
}

fn end(key: &str) {
    sessions().lock().unwrap().remove(key);
}

fn signal(key: &str) {
    if let Some(f) = sessions().lock().unwrap().get(key) {
        f.store(true, Ordering::SeqCst);
    }
}

pub fn mic(state: &AppState) -> Source {
    let s = speech::voice_settings(&state.db.lock().unwrap());
    Source::Mic { device: s.mic, voice: s.voice_processing }
}

// ---------- dictation ----------

/// Turns speech into text phrase by phrase until `stop`, then finishes what
/// was said last. `emit` gets ("text", {text}) for each phrase.
pub async fn dictate(
    ep: &SpeechEndpoint,
    mut rx: mpsc::UnboundedReceiver<Vec<f32>>,
    stop: &AtomicBool,
    level: &(dyn Fn() -> f32 + Sync),
    emit: &(dyn Fn(&str, Value) + Sync),
) -> Result<(), String> {
    let mut phraser = Phraser::new(vad::Config { end_ms: 700, ..Default::default() }, 20.0);
    let mut so_far = String::new();
    let started = Instant::now();
    let mut last_level = Instant::now();
    let handle = |u: vad::Utterance, so_far: &str| {
        let opts = speech::Options { prompt: Some(so_far.to_string()), ..Default::default() };
        let samples = u.samples;
        async move { speech::transcribe(ep, &samples, &opts).await }
    };
    loop {
        if stop.load(Ordering::SeqCst) || started.elapsed() > MAX_DICTATION {
            break;
        }
        let chunk = match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some(c)) => c,
            Ok(None) => break, // the microphone stopped
            Err(_) => Vec::new(),
        };
        if last_level.elapsed() >= Duration::from_millis(100) {
            emit("level", json!({ "level": level() }));
            last_level = Instant::now();
        }
        for p in phraser.push(&chunk) {
            match p {
                Phrase::Started => emit("hearing", json!({})),
                Phrase::Done(u) => {
                    let t = handle(u, &so_far).await?;
                    if !t.text.is_empty() {
                        so_far.push(' ');
                        so_far.push_str(&t.text);
                        emit("text", json!({ "text": t.text }));
                    }
                    emit("listening", json!({}));
                }
            }
        }
    }
    // Whatever was still arriving when the user stopped.
    while let Ok(c) = rx.try_recv() {
        for p in phraser.push(&c) {
            if let Phrase::Done(u) = p {
                let t = handle(u, &so_far).await?;
                if !t.text.is_empty() {
                    so_far.push(' ');
                    so_far.push_str(&t.text);
                    emit("text", json!({ "text": t.text }));
                }
            }
        }
    }
    if let Some(u) = phraser.flush() {
        let t = handle(u, &so_far).await?;
        if !t.text.is_empty() {
            emit("text", json!({ "text": t.text }));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn start_dictation(app: AppHandle, state: AppStateRef<'_>) -> Result<String, String> {
    crate::features::require(&state, crate::features::Feature::Dictation)?;
    let id = uuid::Uuid::new_v4().to_string();
    let key = format!("dictation:{id}");
    let stop = begin(&key)?;
    let state = state.inner().clone();
    let sid = id.clone();
    tauri::async_runtime::spawn(async move {
        let emit = |kind: &str, mut payload: Value| {
            payload["id"] = json!(sid);
            payload["state"] = json!(kind);
            app.emit("dictation", payload).ok();
        };
        let _hold = speech::hold(&state);
        emit("loading", json!({}));
        let result: Result<(), String> = async {
            let ep = speech::endpoint(&state, speech::Use::Live).await?;
            let (tx, rx) = mpsc::unbounded_channel();
            let capture = Capture::start(mic(&state), tx)?;
            emit("listening", json!({}));
            let level = || capture.level();
            let r = dictate(&ep, rx, &stop, &level, &emit).await;
            let device_error = capture.error();
            drop(capture);
            r?;
            device_error.map_or(Ok(()), Err)
        }
        .await;
        end(&key);
        match result {
            Ok(()) => emit("done", json!({})),
            Err(e) => emit("error", json!({ "error": e })),
        }
    });
    Ok(id)
}

#[tauri::command]
pub fn stop_dictation(id: String) {
    signal(&format!("dictation:{id}"));
}

// ---------- speaking ----------

/// Language code from whisper's language name, for picking a voice.
pub fn language_code(name: &str) -> Option<&'static str> {
    const NAMES: &[(&str, &str)] = &[
        ("english", "en"), ("spanish", "es"), ("french", "fr"), ("german", "de"), ("italian", "it"),
        ("portuguese", "pt"), ("dutch", "nl"), ("polish", "pl"), ("russian", "ru"), ("ukrainian", "uk"),
        ("turkish", "tr"), ("arabic", "ar"), ("hindi", "hi"), ("chinese", "zh"), ("japanese", "ja"),
        ("korean", "ko"), ("vietnamese", "vi"), ("swedish", "sv"), ("norwegian", "nb"), ("danish", "da"),
        ("finnish", "fi"), ("czech", "cs"), ("greek", "el"), ("hebrew", "he"), ("indonesian", "id"),
    ];
    let n = name.trim().to_ascii_lowercase();
    NAMES.iter().find(|(k, c)| *k == n || *c == n).map(|(_, c)| *c)
}

/// Speaks sentences one after another on the PC's speakers. Dropping the
/// sender, or bumping `generation`, ends it.
pub struct Speaker {
    tx: mpsc::UnboundedSender<String>,
    generation: Arc<AtomicU64>,
}

impl Speaker {
    pub fn start(state: &Arc<AppState>, language: Option<String>) -> Speaker {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let generation = Arc::new(AtomicU64::new(0));
        let (state, gen) = (state.clone(), generation.clone());
        let settings = speech::voice_settings(&state.db.lock().unwrap());
        tauri::async_runtime::spawn(async move {
            let voices = tokio::task::spawn_blocking(tts::voices).await.ok().and_then(Result::ok).unwrap_or_default();
            let code = language.as_deref().and_then(language_code).or_else(|| language_code(&settings.language));
            let voice = tts::pick_voice(&voices, settings.voice.as_deref(), code).map(|v| v.id.clone());
            if voice.as_deref().is_some_and(|v| v.starts_with("supertonic:")) {
                let _ = tokio::task::spawn_blocking(crate::natural::warm).await;
            }
            while let Some(text) = rx.recv().await {
                let my_gen = gen.load(Ordering::SeqCst);
                let v = voice.clone();
                let rate = settings.rate;
                let lang = code.map(str::to_string);
                let Ok(Ok(s)) = tokio::task::spawn_blocking(move || tts::synthesize(&text, v.as_deref(), lang.as_deref(), rate)).await else {
                    continue;
                };
                if gen.load(Ordering::SeqCst) == my_gen {
                    state.player.play(s.rate, s.samples);
                }
            }
        });
        Speaker { tx, generation }
    }

    pub fn say(&self, text: String) {
        let _ = self.tx.send(text);
    }

    /// Drops anything not yet spoken (the player is stopped separately).
    pub fn hush(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

#[tauri::command]
pub async fn list_voices() -> Result<Vec<tts::Voice>, String> {
    tokio::task::spawn_blocking(tts::voices).await.map_err(|e| e.to_string())?
}

/// Reads text aloud (a reply, a translation, a voice test).
#[tauri::command]
pub async fn speak(state: AppStateRef<'_>, text: String, language: Option<String>) -> Result<(), String> {
    state.player.stop();
    let settings = speech::voice_settings(&state.db.lock().unwrap());
    let voices = tokio::task::spawn_blocking(tts::voices).await.map_err(|e| e.to_string())??;
    let code = language.as_deref().and_then(language_code).or_else(|| language_code(&settings.language));
    let voice = tts::pick_voice(&voices, settings.voice.as_deref(), code)
        .map(|v| v.id.clone())
        .ok_or("No voices are installed in Windows.")?;
    let mut sentences = tts::Sentences::default();
    let mut parts = sentences.push(&text);
    parts.extend(sentences.finish());
    for part in parts {
        let v = voice.clone();
        let rate = settings.rate;
        let lang = code.map(str::to_string);
        let s = tokio::task::spawn_blocking(move || tts::synthesize(&part, Some(&v), lang.as_deref(), rate)).await.map_err(|e| e.to_string())??;
        state.player.play(s.rate, s.samples);
    }
    Ok(())
}

#[tauri::command]
pub fn stop_speaking(state: AppStateRef) {
    state.player.stop();
}

// ---------- voice mode ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    Idle,
    Busy,
}

/// A spoken conversation in one chat: listen, answer aloud, and stop
/// talking as soon as the user speaks again.
#[tauri::command]
pub async fn start_voice(app: AppHandle, state: AppStateRef<'_>, chat_id: String) -> Result<(), String> {
    state.cipher()?;
    crate::features::require(&state, crate::features::Feature::VoiceChat)?;
    let key = format!("voice:{chat_id}");
    let stop = begin(&key)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let emit = {
            let (app, chat_id) = (app.clone(), chat_id.clone());
            move |kind: &str, mut payload: Value| {
                payload["chat_id"] = json!(chat_id);
                payload["state"] = json!(kind);
                app.emit("voice", payload).ok();
            }
        };
        let _hold = speech::hold(&state);
        emit("loading", json!({}));
        let result = voice_loop(&app, &state, &chat_id, &stop, &emit).await;
        state.player.stop();
        end(&key);
        match result {
            Ok(()) => emit("ended", json!({})),
            Err(e) => emit("error", json!({ "error": e })),
        }
    });
    Ok(())
}

async fn voice_loop(
    app: &AppHandle,
    state: &Arc<AppState>,
    chat_id: &str,
    stop: &AtomicBool,
    emit: &(dyn Fn(&str, Value) + Send + Sync),
) -> Result<(), String> {
    let ep = speech::endpoint(state, speech::Use::Live).await?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let capture = Capture::start(mic(state), tx)?;
    let mut phraser = Phraser::new(vad::Config { end_ms: 750, ..Default::default() }, 30.0);
    let turn_state = Arc::new(Mutex::new(Turn::Idle));
    let mut queue: Vec<vad::Utterance> = Vec::new();
    let mut speaker: Option<Arc<Speaker>> = None;
    let mut last_level = Instant::now();
    // What it said lately, to recognize its own voice coming back.
    let spoken = Arc::new(Mutex::new(String::new()));
    emit("listening", json!({}));

    while !stop.load(Ordering::SeqCst) {
        let chunk = match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some(c)) => c,
            Ok(None) => return Err(capture.error().unwrap_or_else(|| "The microphone stopped.".into())),
            Err(_) => Vec::new(),
        };
        let busy = *turn_state.lock().unwrap() == Turn::Busy;
        let talking = state.player.busy();
        // While the PC talks, its own voice may come back through the
        // microphone; only clearly louder speech counts as the user.
        phraser.vad.boost = if talking { 3.0 } else { 1.0 };
        if last_level.elapsed() >= Duration::from_millis(100) {
            emit("level", json!({ "level": capture.level(), "speaking": talking }));
            last_level = Instant::now();
        }
        for p in phraser.push(&chunk) {
            match p {
                Phrase::Started => {
                    if busy || talking {
                        // Barge-in: stop talking and stop the reply.
                        state.player.stop();
                        if let Some(s) = &speaker {
                            s.hush();
                        }
                        if let Some(flag) = state.generations.lock().unwrap().get(chat_id) {
                            flag.store(true, Ordering::Relaxed);
                        }
                        emit("interrupted", json!({}));
                    }
                    emit("hearing", json!({}));
                }
                Phrase::Done(u) => queue.push(u),
            }
        }
        if *turn_state.lock().unwrap() == Turn::Busy || queue.is_empty() || phraser.speaking() {
            continue;
        }
        // Everything said since the last answer, as one message.
        let samples: Vec<f32> = queue.drain(..).flat_map(|u| u.samples).collect();
        emit("thinking", json!({}));
        let heard = speech::transcribe(&ep, &samples, &speech::Options::default()).await?;
        let text = heard.text.trim().to_string();
        let echo = crate::meeting::is_echo(&text, &spoken.lock().unwrap());
        if text.is_empty() || echo {
            emit("listening", json!({}));
            continue;
        }
        spoken.lock().unwrap().clear();
        emit("heard", json!({ "text": text }));
        let s = Arc::new(Speaker::start(state, heard.language.clone()));
        speaker = Some(s.clone());
        *turn_state.lock().unwrap() = Turn::Busy;
        let sentences = Arc::new(Mutex::new(tts::Sentences::default()));
        let chat = chat_id.to_string();
        let tee: crate::Tee = {
            let (s, sentences, chat, spoken) = (s.clone(), sentences.clone(), chat.clone(), spoken.clone());
            Arc::new(move |event: &str, payload: &Value| {
                if event == "chat:delta" && payload["chat_id"] == chat.as_str() {
                    if let Some(t) = payload["content"].as_str() {
                        spoken.lock().unwrap().push_str(t);
                        for sentence in sentences.lock().unwrap().push(t) {
                            s.say(sentence);
                        }
                    }
                } else if event == "chat:start" && payload["chat_id"] == chat.as_str() {
                    // A new step after tool calls: anything half-said so far
                    // belongs to the previous step.
                    for sentence in sentences.lock().unwrap().finish() {
                        s.say(sentence);
                    }
                }
            })
        };
        let (app, state2, ts, emit_busy) = (app.clone(), state.clone(), turn_state.clone(), sentences.clone());
        tauri::async_runtime::spawn(async move {
            let r = crate::run_turn(&app, &state2, chat, text, Some(tee)).await;
            for sentence in emit_busy.lock().unwrap().finish() {
                s.say(sentence);
            }
            if let Err(e) = r {
                eprintln!("voice turn failed: {e}");
            }
            *ts.lock().unwrap() = Turn::Idle;
        });
        emit("speaking", json!({}));
    }
    // Let a running reply finish in the chat, silently.
    state.player.stop();
    Ok(())
}

#[tauri::command]
pub fn stop_voice(state: AppStateRef, chat_id: String) {
    signal(&format!("voice:{chat_id}"));
    state.player.stop();
}

/// "Tap to interrupt": stop talking and stop the reply, keep listening.
#[tauri::command]
pub fn interrupt_voice(state: AppStateRef, chat_id: String) {
    state.player.stop();
    if let Some(flag) = state.generations.lock().unwrap().get(&chat_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_language_names_map_to_codes() {
        assert_eq!(language_code("English"), Some("en"));
        assert_eq!(language_code("de"), Some("de"));
        assert_eq!(language_code("klingon"), None);
    }

    #[test]
    fn sessions_are_exclusive_per_key() {
        let a = begin("test:x").unwrap();
        assert!(begin("test:x").is_err());
        signal("test:x");
        assert!(a.load(Ordering::SeqCst));
        end("test:x");
        assert!(begin("test:x").is_ok());
        end("test:x");
    }
}

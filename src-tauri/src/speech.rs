// SPDX-License-Identifier: AGPL-3.0-only
//! Speech recognition with whisper.cpp, run as a private local process.
//!
//! `whisper-server` has no API-key option, so every route sits behind a random
//! path prefix (`--request-path`), on a random port, on 127.0.0.1 only, and it
//! serves files from an empty folder. Other programs on the PC can't use it
//! without the prefix, which only this app knows.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::process::{Child, Command};

use crate::audio;
use crate::catalog::{Asset, Budget, EngineSpec, License};
use crate::db;
use crate::download;
use crate::engine::{self, JobRef};
use crate::hardware::Hardware;
use crate::net;
use crate::paths::Paths;
use crate::{AppState, AppStateRef};

const SERVER_EXE: &str = if cfg!(windows) { "whisper-server.exe" } else { "whisper-server" };
const MIB: u64 = 1024 * 1024;
/// Slowest speed (times faster than real time) still offered. Meetings
/// transcribe two channels as they happen, so anything slower falls behind.
pub const MIN_SPEED: f64 = 1.5;
/// Unload the speech model after this long unused.
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);

// ---------- catalog ----------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpeechCatalog {
    pub engine: EngineSpec,
    pub vad: VadFile,
    pub models: Vec<SpeechModel>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VadFile {
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub license: License,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpeechModel {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub source: String,
    pub description: String,
    pub license: License,
    pub languages: u32,
    /// Seconds to encode one 30 s window on 8 modern cores (a rough guide).
    pub secs_per_30s: f64,
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

impl SpeechModel {
    fn asset(&self) -> Asset {
        Asset { url: self.url.clone(), size: self.size, sha256: self.sha256.clone() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeechFit {
    /// Estimated times faster than real time.
    pub speed: f64,
    pub needed_bytes: u64,
    pub runnable: bool,
    pub disk_ok: bool,
}

/// Threads for transcription: about two thirds of the cores, leaving room
/// for the rest of the PC (a call app, the browser).
pub fn threads(hw: &Hardware) -> usize {
    let cores = hw.cpu_cores.unwrap_or(hw.cpu_threads / 2).max(1);
    (cores * 2 / 3).clamp(2, 16)
}

pub fn fit(m: &SpeechModel, hw: &Hardware, b: &Budget) -> SpeechFit {
    // Doubling threads saves about 40% here, not 50%.
    let mut secs = m.secs_per_30s * (8.0 / threads(hw) as f64).powf(0.75);
    if !hw.avx2 {
        secs *= 2.5;
    }
    let speed = ((30.0 / secs) * 10.0).round() / 10.0;
    let needed = m.size * 2 + 300 * MIB;
    SpeechFit { speed, needed_bytes: needed, runnable: speed >= MIN_SPEED && needed <= b.ram, disk_ok: m.size <= b.disk }
}

/// The most accurate model that is comfortably fast, else the fastest.
pub fn recommended<'a>(models: &'a [SpeechModel], hw: &Hardware, b: &Budget) -> Option<&'a SpeechModel> {
    let ok: Vec<(&SpeechModel, SpeechFit)> =
        models.iter().map(|m| (m, fit(m, hw, b))).filter(|(_, f)| f.runnable && f.disk_ok).collect();
    ok.iter()
        .rev()
        .find(|(_, f)| f.speed >= 3.0)
        .or_else(|| ok.iter().max_by(|a, b| a.1.speed.total_cmp(&b.1.speed)))
        .map(|(m, _)| *m)
}

// ---------- installed models & settings ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSpeech {
    pub model_id: String,
    pub path: String,
    pub size: u64,
    pub installed_at: i64,
    /// Measured times faster than real time.
    pub speed: Option<f64>,
}

pub fn installed(conn: &Connection) -> Vec<InstalledSpeech> {
    db::get(conn, "speech_installed").unwrap_or_default()
}

fn save_installed(conn: &Connection, list: &[InstalledSpeech]) -> Result<(), String> {
    db::set(conn, "speech_installed", &list)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Model for meetings, where accuracy matters most (None = automatic).
    pub speech_model: Option<String>,
    /// Model for dictation and voice chats, where speed matters most
    /// (None = automatic).
    pub live_model: Option<String>,
    /// Microphone device id (None = the Windows default).
    pub mic: Option<String>,
    /// Ask Windows for echo cancellation and noise suppression.
    pub voice_processing: bool,
    /// Spoken language for recognition, or "auto".
    pub language: String,
    /// Voice for speaking replies (None = a voice matching the language).
    pub voice: Option<String>,
    /// Speaking speed, 1.0 = normal.
    pub rate: f64,
    /// Keep meeting audio (encrypted) so it can be replayed.
    pub keep_meeting_audio: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            speech_model: None,
            live_model: None,
            mic: None,
            voice_processing: true,
            language: "auto".into(),
            voice: None,
            rate: 1.0,
            keep_meeting_audio: true,
        }
    }
}

pub fn voice_settings(conn: &Connection) -> VoiceSettings {
    db::get(conn, "voice").unwrap_or_default()
}

pub fn update_voice_settings(conn: &Connection, f: impl FnOnce(&mut VoiceSettings)) -> Result<VoiceSettings, String> {
    let mut s = voice_settings(conn);
    f(&mut s);
    db::set(conn, "voice", &s)?;
    Ok(s)
}

// ---------- the server ----------

struct Running {
    child: Child,
    port: u16,
    prefix: String,
    model_id: String,
}

#[derive(Debug, Clone)]
pub struct SpeechEndpoint {
    port: u16,
    prefix: String,
    /// Default spoken language ("auto" detects it).
    pub language: String,
}

impl SpeechEndpoint {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}{path}", self.port, self.prefix)
    }

    #[cfg(test)]
    pub fn port_for_test(&self) -> u16 {
        self.port
    }
}

#[derive(Default)]
struct Server {
    running: Option<Running>,
}

impl Server {
    fn alive(&mut self) -> bool {
        match self.running.as_mut() {
            Some(r) => matches!(r.child.try_wait(), Ok(None)),
            None => false,
        }
    }

    async fn ensure(&mut self, l: Launch<'_>, job: Option<&JobRef>) -> Result<(u16, String), String> {
        if self.alive() {
            let r = self.running.as_ref().unwrap();
            if r.model_id == l.model_id {
                return Ok((r.port, r.prefix.clone()));
            }
        }
        self.stop().await;

        let port = engine::free_port()?;
        let prefix = format!("/{}", uuid::Uuid::new_v4().simple());
        std::fs::create_dir_all(l.public).map_err(|e| e.to_string())?;
        let log = std::fs::File::create(l.log).map_err(|e| e.to_string())?;
        let log_err = log.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(l.exe);
        cmd.arg("-m").arg(l.model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--request-path", &prefix])
            .arg("--public").arg(l.public)
            .args(["-t", &l.threads.to_string(), "-l", "auto", "-nlp", "-sns"])
            .arg("--vad").arg("-vm").arg(l.vad)
            .current_dir(l.public)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| format!("Couldn't start speech recognition: {e}"))?;
        #[cfg(windows)]
        if let (Some(job), Some(pid)) = (job, child.id()) {
            job.assign(pid).ok();
        }
        #[cfg(not(windows))]
        let _ = job;
        self.running = Some(Running { child, port, prefix: prefix.clone(), model_id: l.model_id.to_string() });

        let client = net::local_client();
        let health = format!("http://127.0.0.1:{port}{prefix}/health");
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(120) {
            if !self.alive() {
                self.running = None;
                return Err(format!("Speech recognition stopped while loading.\n{}", engine::log_tail(l.log, 8)));
            }
            if let Ok(r) = client.get(&health).timeout(Duration::from_secs(2)).send().await {
                if r.status().is_success() {
                    return Ok((port, prefix));
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        self.stop().await;
        Err("The speech model took too long to load.".into())
    }

    async fn stop(&mut self) {
        if let Some(mut r) = self.running.take() {
            r.child.start_kill().ok();
            let _ = tokio::time::timeout(Duration::from_secs(5), r.child.wait()).await;
        }
    }
}

struct Launch<'a> {
    exe: &'a Path,
    model: &'a Path,
    model_id: &'a str,
    vad: &'a Path,
    public: &'a Path,
    threads: usize,
    log: &'a Path,
}

/// Speech recognition state shared by dictation, voice mode and meetings.
pub struct Speech {
    /// One server per loaded model, so a meeting and a dictation can run at once.
    servers: tokio::sync::Mutex<std::collections::HashMap<String, Server>>,
    users: AtomicUsize,
    last_used: Mutex<Instant>,
}

impl Default for Speech {
    fn default() -> Self {
        Speech { servers: Default::default(), users: AtomicUsize::new(0), last_used: Mutex::new(Instant::now()) }
    }
}

/// Keeps the speech model loaded while a dictation, voice chat or meeting runs.
pub struct InUse(Arc<AppState>);

impl Drop for InUse {
    fn drop(&mut self) {
        self.0.speech.users.fetch_sub(1, Ordering::SeqCst);
        *self.0.speech.last_used.lock().unwrap() = Instant::now();
    }
}

pub fn hold(state: &Arc<AppState>) -> InUse {
    state.speech.users.fetch_add(1, Ordering::SeqCst);
    InUse(state.clone())
}

fn engine_dir(spec: &EngineSpec) -> String {
    format!("whisper-{}-cpu-x64", spec.build)
}

fn vad_path(paths: &Paths, c: &SpeechCatalog) -> PathBuf {
    paths.models.join("whisper-vad").join(&c.vad.file)
}

fn model_path(paths: &Paths, m: &InstalledSpeech) -> PathBuf {
    let stored = PathBuf::from(&m.path);
    if stored.exists() {
        return stored;
    }
    match stored.file_name() {
        Some(name) => paths.models.join(&m.model_id).join(name),
        None => stored,
    }
}

/// What the transcription is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// Dictation and voice chats: answer within a second or two.
    Live,
    /// Meetings: the most accurate model that keeps up.
    Accurate,
}

/// A live model should encode a 30 s window in about this long, or less.
const LIVE_WINDOW_SECS: f64 = 2.5;

fn window_secs(m: &SpeechModel, hw: &Hardware) -> f64 {
    30.0 / fit(m, hw, &Budget::from_hardware(hw)).speed.max(0.01)
}

/// Picks the installed model for a use: the user's choice if set, else the
/// most accurate (meetings) or the most accurate that is quick enough (live).
pub fn choose(list: &[InstalledSpeech], s: &VoiceSettings, models: &[SpeechModel], hw: &Hardware, purpose: Use) -> Option<InstalledSpeech> {
    let chosen = match purpose {
        Use::Live => s.live_model.as_ref(),
        Use::Accurate => s.speech_model.as_ref(),
    };
    if let Some(m) = chosen.and_then(|id| list.iter().find(|m| &m.model_id == id)) {
        return Some(m.clone());
    }
    // Catalog order runs from fastest to most accurate.
    let ordered: Vec<(&InstalledSpeech, &SpeechModel)> =
        models.iter().filter_map(|spec| list.iter().find(|m| m.model_id == spec.id).map(|m| (m, spec))).collect();
    let pick = match purpose {
        Use::Accurate => ordered.last(),
        Use::Live => ordered.iter().rev().find(|(_, spec)| window_secs(spec, hw) <= LIVE_WINDOW_SECS).or(ordered.first()),
    };
    pick.map(|(m, _)| (*m).clone()).or_else(|| list.first().cloned())
}

/// The running speech endpoint for a use, starting its model if needed.
pub async fn endpoint(state: &AppState, purpose: Use) -> Result<SpeechEndpoint, String> {
    let (settings, list) = {
        let conn = state.db.lock().unwrap();
        (voice_settings(&conn), installed(&conn))
    };
    let hw = state.hardware.read().unwrap().clone();
    let chosen = choose(&list, &settings, &state.catalog.speech.models, &hw, purpose)
        .ok_or("Install a speech model first: open Models › Speech.")?;
    endpoint_for(state, &chosen, settings.language).await
}

async fn endpoint_for(state: &AppState, chosen: &InstalledSpeech, language: String) -> Result<SpeechEndpoint, String> {
    let exe = engine::find_exe(&state.paths.engines.join(engine_dir(&state.catalog.speech.engine)), SERVER_EXE)
        .ok_or("Speech recognition isn't fully installed. Reinstall the speech model from Models › Speech.")?;
    let vad = vad_path(&state.paths, &state.catalog.speech);
    let model = model_path(&state.paths, chosen);
    let threads = threads(&state.hardware.read().unwrap());
    *state.speech.last_used.lock().unwrap() = Instant::now();
    let mut servers = state.speech.servers.lock().await;
    // At most two models stay loaded; drop the others.
    let others: Vec<String> = servers.keys().filter(|k| *k != &chosen.model_id).cloned().collect();
    if others.len() >= 2 {
        for k in others {
            if let Some(mut s) = servers.remove(&k) {
                s.stop().await;
            }
        }
    }
    let server = servers.entry(chosen.model_id.clone()).or_default();
    let (port, prefix) = server
        .ensure(
            Launch {
                exe: &exe,
                model: &model,
                model_id: &chosen.model_id,
                vad: &vad,
                public: &state.paths.engines.join("whisper-public"),
                threads,
                log: &state.paths.logs.join(format!("speech-{}.log", chosen.model_id)),
            },
            state.job(),
        )
        .await?;
    Ok(SpeechEndpoint { port, prefix, language })
}

pub async fn stop(state: &AppState) {
    let mut servers = state.speech.servers.lock().await;
    for (_, mut s) in servers.drain() {
        s.stop().await;
    }
}

/// Unloads the speech model when nothing has used it for a while.
pub fn start_idle_unloader(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let idle = state.speech.last_used.lock().unwrap().elapsed() > IDLE_UNLOAD;
            if idle && state.speech.users.load(Ordering::SeqCst) == 0 {
                stop(&state).await;
            }
            crate::natural::unload_if_idle();
        }
    });
}

// ---------- transcription ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Segment {
    /// Seconds from the start of the audio sent.
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Transcript {
    pub text: String,
    /// The language heard, as whisper names it ("english").
    pub language: Option<String>,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Overrides the endpoint's language ("auto" or a code such as "en").
    pub language: Option<String>,
    /// Translate into English while transcribing.
    pub translate: bool,
    /// Earlier text, to keep names and spelling consistent.
    pub prompt: Option<String>,
}

/// Text whisper writes for sounds rather than speech, e.g. "[BLANK_AUDIO]".
fn is_annotation(t: &str) -> bool {
    let t = t.trim();
    t.is_empty()
        || (t.starts_with('[') && t.ends_with(']'))
        || (t.starts_with('(') && t.ends_with(')'))
        || (t.starts_with('*') && t.ends_with('*'))
        || t.chars().all(|c| !c.is_alphanumeric())
}

pub fn clean(t: Transcript) -> Transcript {
    let segments: Vec<Segment> = t
        .segments
        .into_iter()
        .filter(|s| !is_annotation(&s.text))
        .map(|s| Segment { text: s.text.trim().to_string(), ..s })
        .collect();
    let text = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
    Transcript { text, language: t.language, segments }
}

/// Transcribes 16 kHz mono samples.
pub async fn transcribe(ep: &SpeechEndpoint, samples: &[f32], opts: &Options) -> Result<Transcript, String> {
    if samples.len() < (audio::RATE / 4) as usize {
        return Ok(Transcript::default());
    }
    let wav = audio::wav_encode(samples, audio::RATE);
    let part = reqwest::multipart::Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav").map_err(|e| e.to_string())?;
    let language = opts.language.clone().unwrap_or_else(|| ep.language.clone());
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("response_format", "verbose_json")
        .text("temperature", "0.0")
        .text("language", if language.is_empty() { "auto".into() } else { language })
        .text("translate", if opts.translate { "true" } else { "false" });
    if let Some(p) = opts.prompt.as_ref().filter(|p| !p.trim().is_empty()) {
        form = form.text("prompt", p.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect::<String>());
    }
    let resp = net::local_client()
        .post(ep.url("/inference"))
        .multipart(form)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach speech recognition: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| format!("Speech recognition sent an unreadable answer: {e}"))?;
    if !status.is_success() || v.get("error").is_some() {
        return Err(format!("Speech recognition failed: {}", v.get("error").and_then(Value::as_str).unwrap_or("unknown error")));
    }
    let segments: Vec<Segment> = v["segments"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| Segment {
                    start: s["start"].as_f64().unwrap_or(0.0),
                    end: s["end"].as_f64().unwrap_or(0.0),
                    text: s["text"].as_str().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(clean(Transcript {
        text: v["text"].as_str().unwrap_or_default().trim().to_string(),
        language: v["language"].as_str().map(str::to_string),
        segments,
    }))
}

// ---------- install ----------

/// Measures speed with a sentence spoken by a Windows voice. Also proves the
/// whole chain works. Returns (times faster than real time, what was heard).
async fn self_test(state: &AppState, model: &InstalledSpeech) -> Result<(f64, String), String> {
    const SENTENCE: &str = "Meeting notes: we agreed to move the product launch to Thursday, \
                            and Jordan will send the updated budget by Friday afternoon.";
    let speech = tokio::task::spawn_blocking(|| crate::tts::synthesize(SENTENCE, Some(crate::tts::WINDOWS_DEFAULT), Some("en"), 1.0))
        .await
        .map_err(|e| e.to_string())??;
    let floats: Vec<f32> = speech.samples.iter().map(|s| *s as f32 / 32768.0).collect();
    let samples = audio::resample(&floats, speech.rate, audio::RATE);
    let ep = endpoint_for(state, model, "en".into()).await?;
    let opts = Options::default();
    // The first request warms the model up; time the second.
    transcribe(&ep, &samples, &opts).await?;
    let started = Instant::now();
    let heard = transcribe(&ep, &samples, &opts).await?;
    let secs = samples.len() as f64 / audio::RATE as f64;
    Ok(((secs / started.elapsed().as_secs_f64() * 10.0).round() / 10.0, heard.text))
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct SpeechCard {
    #[serde(flatten)]
    spec: SpeechModel,
    fit: SpeechFit,
    installed: Option<InstalledSpeech>,
    recommended: bool,
}

#[derive(Serialize)]
pub struct SpeechView {
    models: Vec<SpeechCard>,
    hidden: usize,
    /// Models in use now (after any automatic choice): meetings, and live use.
    active: Option<String>,
    live: Option<String>,
    /// Suggested for live use, when it differs from the main suggestion.
    recommended_live: Option<String>,
    threads: usize,
    installing: Vec<String>,
}

#[tauri::command]
pub fn speech_view(state: AppStateRef) -> SpeechView {
    let hw = state.hardware.read().unwrap().clone();
    let b = Budget::from_hardware(&hw);
    let (list, settings) = {
        let conn = state.db.lock().unwrap();
        (installed(&conn), voice_settings(&conn))
    };
    let models = &state.catalog.speech.models;
    let rec = recommended(models, &hw, &b).map(|m| m.id.clone());
    let mut hidden = 0;
    let cards = models
        .iter()
        .filter_map(|m| {
            let f = fit(m, &hw, &b);
            let inst = list.iter().find(|i| i.model_id == m.id).cloned();
            if inst.is_none() && !(f.runnable && f.disk_ok) {
                hidden += 1;
                return None;
            }
            Some(SpeechCard { recommended: rec.as_deref() == Some(m.id.as_str()), spec: m.clone(), fit: f, installed: inst })
        })
        .collect();
    let active = choose(&list, &settings, models, &hw, Use::Accurate).map(|m| m.model_id);
    let live = choose(&list, &settings, models, &hw, Use::Live).map(|m| m.model_id);
    let recommended_live = models
        .iter()
        .rev()
        .find(|m| {
            let f = fit(m, &hw, &b);
            f.runnable && f.disk_ok && window_secs(m, &hw) <= LIVE_WINDOW_SECS
        })
        .map(|m| m.id.clone())
        .filter(|id| Some(id) != rec.as_ref());
    SpeechView {
        models: cards,
        hidden,
        active,
        live,
        recommended_live,
        threads: threads(&hw),
        installing: state.installs.lock().unwrap().keys().cloned().collect(),
    }
}

#[tauri::command]
pub fn install_speech_model(app: AppHandle, state: AppStateRef, model_id: String) -> Result<(), String> {
    let spec = state.catalog.speech.models.iter().find(|m| m.id == model_id).ok_or("Unknown speech model")?.clone();
    let hw = state.hardware.read().unwrap().clone();
    let f = fit(&spec, &hw, &Budget::from_hardware(&hw));
    if !f.runnable {
        return Err("This PC can't run that speech model quickly enough.".into());
    }
    if !f.disk_ok {
        return Err("There isn't enough free disk space for this model.".into());
    }
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = crate::claim(&state.installs, &model_id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
            app.emit("install:progress", json!({ "model_id": model_id, "phase": phase, "received": received, "total": total })).ok();
        };
        state.log("model", &format!("Started installing {}", spec.name));
        let result = install(&state, &spec, &cancel, &emit).await;
        let payload = match &result {
            Ok(speed) => {
                state.log("model", &format!("Installed {}", spec.name));
                json!({ "model_id": model_id, "name": spec.name, "ok": true, "speed": speed })
            }
            Err(e) if e == download::CANCELLED => {
                state.log("model", &format!("Cancelled installing {}", spec.name));
                json!({ "model_id": model_id, "name": spec.name, "ok": false, "cancelled": true })
            }
            Err(e) => {
                state.log("model", &format!("Installing {} failed: {e}", spec.name));
                json!({ "model_id": model_id, "name": spec.name, "ok": false, "error": e })
            }
        };
        app.emit("install:finished", payload).ok();
    });
    Ok(())
}

async fn install(state: &Arc<AppState>, spec: &SpeechModel, cancel: &AtomicBool, emit: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<Option<f64>, String> {
    let cat = &state.catalog.speech;
    let asset = cat.engine.assets.get("cpu-x64").ok_or("No speech engine for this PC")?;
    let dir = engine_dir(&cat.engine);
    emit("engine", 0, 0);
    if engine::find_exe(&state.paths.engines.join(&dir), SERVER_EXE).is_none() {
        state.log("network", &format!("Downloading the whisper.cpp speech engine from {}", crate::host_of(&asset.url)));
    }
    engine::ensure_unpacked(&state.paths, &dir, asset, SERVER_EXE, cancel, |r, t| emit("engine", r, t)).await?;

    let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
    let vad = vad_path(&state.paths, cat);
    if !vad.exists() {
        state.log("network", &format!("Downloading the Silero voice detector from {}", crate::host_of(&cat.vad.url)));
    }
    download::fetch_verified(&client, &cat.vad.url, &vad, cat.vad.size, &cat.vad.sha256, cancel, |_, _| {}).await?;

    let dest = state.paths.models.join(&spec.id).join(&spec.file);
    if !dest.exists() {
        state.log("network", &format!("Downloading {} ({}) from {}", spec.name, crate::size_label(spec.size), crate::host_of(&spec.url)));
    }
    let a = spec.asset();
    download::fetch_verified(&client, &a.url, &dest, a.size, &a.sha256, cancel, |r, t| emit("download", r, t)).await?;
    let entry = InstalledSpeech {
        model_id: spec.id.clone(),
        path: dest.display().to_string(),
        size: spec.size,
        installed_at: db::now_ms(),
        speed: None,
    };
    {
        let conn = state.db.lock().unwrap();
        let mut list = installed(&conn);
        list.retain(|m| m.model_id != spec.id);
        list.push(entry.clone());
        save_installed(&conn, &list)?;
    }

    emit("benchmark", 0, 0);
    match self_test(state, &entry).await {
        Ok((speed, heard)) => {
            let conn = state.db.lock().unwrap();
            let mut list = installed(&conn);
            if let Some(m) = list.iter_mut().find(|m| m.model_id == spec.id) {
                m.speed = Some(speed);
            }
            save_installed(&conn, &list)?;
            eprintln!("speech self-test: {speed}x real time; heard: {heard}");
            Ok(Some(speed))
        }
        Err(e) => {
            // Installed; only the speed test failed (for example, no Windows voice).
            eprintln!("speech self-test failed: {e}");
            Ok(None)
        }
    }
}

#[cfg(test)]
pub async fn install_for_test(state: &Arc<AppState>, spec: &SpeechModel, cancel: &AtomicBool) -> Result<Option<f64>, String> {
    install(state, spec, cancel, &|_, _, _| {}).await
}

#[tauri::command]
pub async fn remove_speech_model(state: AppStateRef<'_>, model_id: String) -> Result<(), String> {
    if !state.catalog.speech.models.iter().any(|m| m.id == model_id) {
        return Err("Unknown speech model".into());
    }
    stop(&state).await;
    let dir = state.paths.models.join(&model_id);
    if dir.starts_with(&state.paths.models) && dir != state.paths.models && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the model files: {e}"))?;
    }
    let conn = state.db.lock().unwrap();
    let mut list = installed(&conn);
    list.retain(|m| m.model_id != model_id);
    save_installed(&conn, &list)?;
    update_voice_settings(&conn, |s| {
        if s.speech_model.as_deref() == Some(model_id.as_str()) {
            s.speech_model = None;
        }
        if s.live_model.as_deref() == Some(model_id.as_str()) {
            s.live_model = None;
        }
    })?;
    db::log_action(&conn, "model", "Removed a speech model and deleted its files");
    Ok(())
}

#[tauri::command]
pub fn get_voice_settings(state: AppStateRef) -> VoiceSettings {
    voice_settings(&state.db.lock().unwrap())
}

#[tauri::command]
pub async fn set_voice_settings(state: AppStateRef<'_>, settings: VoiceSettings) -> Result<VoiceSettings, String> {
    let list = installed(&state.db.lock().unwrap());
    for id in [&settings.speech_model, &settings.live_model].into_iter().flatten() {
        if !list.iter().any(|m| &m.model_id == id) {
            return Err("That speech model isn't installed.".into());
        }
    }
    update_voice_settings(&state.db.lock().unwrap(), |s| *s = settings)
}

#[derive(Serialize)]
pub struct AudioDevices {
    inputs: Vec<audio::Device>,
    outputs: Vec<audio::Device>,
}

#[tauri::command]
pub async fn audio_devices() -> Result<AudioDevices, String> {
    tokio::task::spawn_blocking(|| {
        Ok(AudioDevices { inputs: audio::devices(audio::Flow::Input)?, outputs: audio::devices(audio::Flow::Output)? })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::GIB;

    fn hw(cores: usize, avx2: bool) -> Hardware {
        Hardware { cpu_cores: Some(cores), cpu_threads: cores * 2, avx2, ram_total: 16 * GIB, disk_free: 100 * GIB, ..Default::default() }
    }

    fn models() -> Vec<SpeechModel> {
        crate::catalog::Catalog::bundled().speech.models
    }

    #[test]
    fn a_strong_pc_gets_the_most_accurate_model() {
        let h = hw(16, true);
        let all = models();
        let r = recommended(&all, &h, &Budget::from_hardware(&h)).unwrap();
        assert_eq!(r.id, "whisper-large-v3-turbo");
    }

    #[test]
    fn a_weak_pc_hides_the_big_model_but_keeps_a_fast_one() {
        let h = hw(2, false);
        let b = Budget::from_hardware(&h);
        let big = models().into_iter().find(|m| m.id == "whisper-large-v3-turbo").unwrap();
        assert!(!fit(&big, &h, &b).runnable);
        let all = models();
        let r = recommended(&all, &h, &b).unwrap();
        assert_eq!(r.id, "whisper-base");
    }

    #[test]
    fn thread_count_leaves_room_for_the_pc() {
        assert_eq!(threads(&hw(4, true)), 2);
        assert_eq!(threads(&hw(8, true)), 5);
        assert_eq!(threads(&hw(24, true)), 16);
        assert_eq!(threads(&hw(1, true)), 2);
    }

    #[test]
    fn meetings_get_the_accurate_model_and_dictation_a_quick_one() {
        let h = hw(24, true);
        let all = models();
        let inst = |id: &str| InstalledSpeech { model_id: id.into(), path: String::new(), size: 0, installed_at: 0, speed: None };
        let list = vec![inst("whisper-large-v3-turbo"), inst("whisper-small")];
        let s = VoiceSettings::default();
        assert_eq!(choose(&list, &s, &all, &h, Use::Accurate).unwrap().model_id, "whisper-large-v3-turbo");
        assert_eq!(choose(&list, &s, &all, &h, Use::Live).unwrap().model_id, "whisper-small");
        // Only the big one installed: it does both jobs.
        let one = vec![inst("whisper-large-v3-turbo")];
        assert_eq!(choose(&one, &s, &all, &h, Use::Live).unwrap().model_id, "whisper-large-v3-turbo");
        // The user's choice wins.
        let s = VoiceSettings { live_model: Some("whisper-large-v3-turbo".into()), ..Default::default() };
        assert_eq!(choose(&list, &s, &all, &h, Use::Live).unwrap().model_id, "whisper-large-v3-turbo");
    }

    #[test]
    fn sound_annotations_are_dropped() {
        let t = Transcript {
            text: String::new(),
            language: None,
            segments: vec![
                Segment { start: 0.0, end: 1.0, text: " [BLANK_AUDIO]".into() },
                Segment { start: 1.0, end: 2.0, text: " Hello there.".into() },
                Segment { start: 2.0, end: 3.0, text: " (music)".into() },
                Segment { start: 3.0, end: 4.0, text: " ...".into() },
            ],
        };
        let c = clean(t);
        assert_eq!(c.text, "Hello there.");
        assert_eq!(c.segments.len(), 1);
    }

    #[test]
    fn settings_default_to_auto_language_and_voice_processing() {
        let s = VoiceSettings::default();
        assert_eq!(s.language, "auto");
        assert!(s.voice_processing && s.keep_meeting_audio);
    }
}

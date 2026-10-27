// SPDX-License-Identifier: AGPL-3.0-only
//! Your voice: the person reads a short script once (lines said in different
//! ways) and sings a little, and SulcusAI then narrates in their voice and
//! sings songs in it.
//!
//! - Narration: Qwen3-TTS 12 Hz Base (Apache-2.0) through qwentts.cpp (MIT,
//!   ggml). It clones from ~12 s of their reading together with its words as
//!   speech recognition heard them ("in-context" cloning), which keeps their
//!   way of speaking, not only their sound.
//! - Singing: a song's voice is taken out (HTDemucs), sung again in theirs by
//!   Seed-VC (`svc`), and put back over the band (`sing`).
//!
//! The recordings are kept like everything else made here: the list
//! encrypted in the settings, each recording sealed in its own file. The
//! person says first that the voice is theirs, or that its owner agreed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use super::{stems, svc, MediaFile};
use crate::audio::{Capture, Source};
use crate::crypto::Cipher;
use crate::paths::Paths;
use crate::{db, download, engine, net, AppState, AppStateRef};

const KEY: &str = "own_voices";
/// Recordings are made at 48 kHz, so the voice keeps its brightness.
const REC_RATE: u32 = 48_000;
/// Qwen3-TTS works at 24 kHz.
pub(crate) const TTS_RATE: u32 = 24_000;
const TTS_EXE: &str = if cfg!(windows) { "qwen-tts.exe" } else { "qwen-tts" };

// ---------- catalog ----------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OwnVoiceCatalog {
    pub speak: SpeakSpec,
    pub sing: SingSpec,
    /// Voices made from a description.
    #[serde(default)]
    pub design: Option<DesignSpec>,
}

/// A voice made from words ("a warm older woman, low and slow"): Qwen3-TTS
/// VoiceDesign, 1.7B only, on the narration engine.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DesignSpec {
    pub name: String,
    pub source: String,
    pub license: crate::catalog::License,
    pub files: Vec<MediaFile>,
}

/// Narration in the person's voice: the engine, the model for a graphics
/// card (1.7B) and the smaller one for the processor (0.6B).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpeakSpec {
    pub name: String,
    pub source: String,
    pub license: crate::catalog::License,
    pub engine: crate::catalog::EngineSpec,
    pub files: Vec<MediaFile>,
    pub processor_files: Vec<MediaFile>,
}

/// Singing in the person's voice: Seed-VC's eight ONNX pieces.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SingSpec {
    pub name: String,
    pub source: String,
    pub license: crate::catalog::License,
    pub files: Vec<MediaFile>,
}

// ---------- the script ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Speak,
    Sing,
}

/// A line of the script: its id, kind, and shortest and longest takes (s).
/// The window shows the words and how to say them, in the person's language.
pub const LINES: &[(&str, LineKind, f32, f32)] = &[
    ("intro", LineKind::Speak, 2.0, 25.0),
    ("everyday", LineKind::Speak, 2.0, 25.0),
    ("story", LineKind::Speak, 3.0, 30.0),
    ("question", LineKind::Speak, 1.5, 20.0),
    ("excited", LineKind::Speak, 1.5, 20.0),
    ("calm", LineKind::Speak, 2.0, 25.0),
    ("sounds", LineKind::Speak, 2.0, 25.0),
    ("numbers", LineKind::Speak, 2.0, 25.0),
    ("hold", LineKind::Sing, 2.5, 20.0),
    ("scale", LineKind::Sing, 3.0, 30.0),
    ("song", LineKind::Sing, 6.0, 75.0),
    ("soft_loud", LineKind::Sing, 3.0, 30.0),
];

fn line(id: &str) -> Option<(LineKind, f32, f32)> {
    LINES.iter().find(|l| l.0 == id).map(|l| (l.1, l.2, l.3))
}

/// The order recordings are joined in for each use, after the person's own
/// recordings they added (clearest first): conversational reading first for
/// narration; singing (a song, then the scale) for songs.
const SPEAK_ORDER: &[&str] = &["everyday", "intro", "story", "question", "excited", "calm", "sounds", "numbers"];
const SING_ORDER: &[&str] = &["song", "scale", "soft_loud", "hold"];
/// Recordings the person adds (`own_voice_upload`): line ids "added-<n>".
const ADDED: &str = "added-";
/// The most recordings a voice keeps from added files.
const MAX_ADDED: usize = 16;
/// How much of each added file is kept: its clearest stretch.
const ADDED_S: f32 = 15.0;
/// An added reading this long is enough for narration alone.
const ENOUGH_SPEECH_S: f32 = 8.0;

// ---------- voices ----------

/// One recording of a line, or of a file the person added.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Take {
    pub line: String,
    pub seconds: f32,
    /// What was said, as speech recognition heard it (else the line's words;
    /// empty for singing and for added readings it couldn't hear).
    pub words: String,
    /// What could be better: "quiet", "loud" (it distorted), "noisy".
    pub notes: Vec<String>,
    pub at: i64,
    /// For an added file: singing or speaking (script lines have theirs).
    pub kind: Option<LineKind>,
    /// For an added file: its name, as shown.
    pub file: String,
    /// For an added file: how clear the voice is in what was kept (0 to 1,
    /// the share of it that's voice); the clearest is used first.
    pub score: f32,
}

impl Take {
    fn kind(&self) -> Option<LineKind> {
        self.kind.or_else(|| line(&self.line).map(|l| l.0))
    }

    fn added(&self) -> bool {
        self.line.starts_with(ADDED)
    }
}

/// A voice: whose it is and its recordings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OwnVoice {
    pub id: String,
    pub name: String,
    pub takes: Vec<Take>,
    pub created_at: i64,
    /// When the person said the voice is theirs or its owner agreed.
    pub agreed_at: i64,
    /// A voice made from a description: the description (its one take is
    /// the reading that was kept).
    pub designed: Option<String>,
}

impl OwnVoice {
    fn takes_of(&self, kind: LineKind) -> impl Iterator<Item = &Take> {
        self.takes.iter().filter(move |t| t.kind() == Some(kind))
    }

    /// Narration needs three lines read, or an added reading of 8 s or more
    /// (a made voice: its reading).
    pub fn can_speak(&self) -> bool {
        self.takes_of(LineKind::Speak).count() >= 3
            || self.takes_of(LineKind::Speak).any(|t| t.added() && (t.seconds >= ENOUGH_SPEECH_S || self.designed.is_some()))
    }

    /// The order to use takes of `kind` in: added files clearest first, then
    /// the script's lines in `script` order.
    fn order(&self, kind: LineKind, script: &[&str]) -> Vec<String> {
        let mut added: Vec<&Take> = self.takes_of(kind).filter(|t| t.added()).collect();
        added.sort_by(|a, b| b.score.total_cmp(&a.score));
        added.into_iter().map(|t| t.line.clone()).chain(script.iter().map(|s| s.to_string())).collect()
    }

    /// Songs work from reading too, and sound more like the person with
    /// some singing.
    pub fn can_sing(&self) -> bool {
        self.takes_of(LineKind::Sing).next().is_some() || self.can_speak()
    }
}

pub fn list(conn: &Connection, c: &Cipher) -> Vec<OwnVoice> {
    let mut all: Vec<OwnVoice> = db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.sort_by_key(|v| v.created_at);
    all
}

fn store(conn: &Connection, c: &Cipher, all: &[OwnVoice]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

pub fn get(conn: &Connection, c: &Cipher, id: &str) -> Option<OwnVoice> {
    list(conn, c).into_iter().find(|v| v.id == id)
}

/// A voice by what a chat called it: its name (or part of it), or "my
/// voice", "me", "mine" for the first one recorded.
pub fn find<'a>(voices: &'a [OwnVoice], name: &str) -> Option<&'a OwnVoice> {
    let n = name.trim().to_lowercase();
    voices
        .iter()
        .find(|v| v.name.trim().to_lowercase() == n)
        .or_else(|| voices.iter().find(|v| !n.is_empty() && v.name.to_lowercase().contains(&n)))
        .or_else(|| matches!(n.as_str(), "my voice" | "my own voice" | "me" | "mine" | "myself" | "the user's voice" | "user").then(|| voices.first()).flatten())
}

fn voices_dir(paths: &Paths) -> PathBuf {
    paths.data.join("voices")
}

fn take_path(paths: &Paths, voice: &str, line: &str) -> Result<PathBuf, String> {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok(voice) || !ok(line) {
        return Err("Bad voice or line.".into());
    }
    Ok(voices_dir(paths).join(voice).join(format!("{line}.sealed")))
}

/// A take's samples (48 kHz mono).
fn read_take(paths: &Paths, c: &Cipher, voice: &str, line: &str) -> Result<Vec<f32>, String> {
    let sealed = std::fs::read(take_path(paths, voice, line)?).map_err(|_| "A recording of this voice is missing; record that line again.".to_string())?;
    let wav = c.open_bytes(&sealed).map_err(|_| "A recording of this voice couldn't be opened.".to_string())?;
    let pcm = crate::audio::wav_decode(&wav)?;
    Ok(stems::resample(&pcm.samples, pcm.rate, REC_RATE))
}

/// Takes joined in `order` (a short pause between them) up to `max_s`, at
/// `rate`, and their words: empty when any joined take's words aren't known
/// (the engine then clones from the sound alone, as wrong words make it run
/// on).
fn joined(paths: &Paths, c: &Cipher, voice: &OwnVoice, order: &[String], rate: u32, max_s: f32) -> Result<(Vec<f32>, String), String> {
    let gap = vec![0f32; (REC_RATE as f32 * 0.3) as usize];
    let (mut out, mut words, mut all_words) = (Vec::new(), Vec::new(), true);
    for id in order {
        let Some(t) = voice.takes.iter().find(|t| t.line == *id) else { continue };
        if !out.is_empty() && (out.len() as f32 + t.seconds * REC_RATE as f32) / REC_RATE as f32 > max_s {
            break;
        }
        if !out.is_empty() {
            out.extend_from_slice(&gap);
        }
        out.extend(read_take(paths, c, &voice.id, id)?);
        all_words &= !t.words.trim().is_empty();
        words.push(t.words.trim().to_string());
    }
    if out.is_empty() {
        return Err("Record a few lines of your voice first (Studio › Your voice).".into());
    }
    out.truncate((max_s * REC_RATE as f32) as usize);
    Ok((stems::resample(&out, REC_RATE, rate), if all_words { words.join(" ") } else { String::new() }))
}

/// What narration clones from: up to 15 s of reading at 24 kHz, and its
/// words. Takes whose words are known come first, and only those when
/// they're enough (6 s): with the words, the engine copies the way the person
/// speaks; without them, only roughly how they sound.
pub fn speech_reference(paths: &Paths, c: &Cipher, voice: &OwnVoice) -> Result<(Vec<f32>, String), String> {
    if !voice.can_speak() {
        return Err("Read at least three lines in Studio › Your voice first.".into());
    }
    let order = voice.order(LineKind::Speak, SPEAK_ORDER);
    let worded: Vec<String> = order.iter().filter(|id| voice.takes.iter().any(|t| t.line == **id && !t.words.trim().is_empty())).cloned().collect();
    let worded_s: f32 = voice.takes.iter().filter(|t| worded.contains(&t.line)).map(|t| t.seconds).sum();
    joined(paths, c, voice, if worded_s >= 6.0 { &worded } else { &order }, TTS_RATE, 15.0)
}

/// Hears the words of a voice's readings that were kept without them (added
/// before speech recognition was here), so narration copies the voice
/// closely from then on.
async fn hear_missing_words(state: &Arc<AppState>, c: &Cipher, voice_id: &str, say: &(dyn Fn(&str) + Sync)) {
    let Some(voice) = get(&state.db.lock().unwrap(), c, voice_id) else { return };
    let missing: Vec<String> = voice.takes.iter().filter(|t| t.kind() == Some(LineKind::Speak) && t.words.trim().is_empty()).map(|t| t.line.clone()).collect();
    for line in missing {
        let Ok(take) = read_take(&state.paths, c, voice_id, &line) else { continue };
        let Some(words) = words_of(state, &take, say).await else { return };
        let conn = state.db.lock().unwrap();
        let mut all = list(&conn, c);
        if let Some(t) = all.iter_mut().find(|v| v.id == voice_id).and_then(|v| v.takes.iter_mut().find(|t| t.line == line)) {
            t.words = words;
        }
        let _ = store(&conn, c, &all);
        crate::applog::line("media", "Heard the words of a voice's reading kept without them");
    }
}

/// How many takes of singing a voice has (what training on the voice learns from).
pub fn sung_count(voice: &OwnVoice) -> usize {
    voice.takes_of(LineKind::Sing).count()
}

/// A voice's singing, take by take (48 kHz mono), clearest added files
/// first: what a custom model learns the voice from.
pub fn sung_takes(paths: &Paths, c: &Cipher, voice: &OwnVoice) -> Result<Vec<Vec<f32>>, String> {
    voice.order(LineKind::Sing, SING_ORDER).iter().filter(|l| voice.takes.iter().any(|t| &t.line == *l)).map(|l| read_take(paths, c, &voice.id, l)).collect()
}

/// What songs are sung like: their singing (else their reading), at 44.1
/// kHz; and whether it was singing.
pub fn singing_reference(paths: &Paths, c: &Cipher, voice: &OwnVoice) -> Result<(Vec<f32>, bool), String> {
    if voice.takes_of(LineKind::Sing).next().is_some() {
        return Ok((joined(paths, c, voice, &voice.order(LineKind::Sing, SING_ORDER), svc::RATE, svc::MAX_PROMPT_S)?.0, true));
    }
    Ok((joined(paths, c, voice, &voice.order(LineKind::Speak, SPEAK_ORDER), svc::RATE, svc::MAX_PROMPT_S)?.0, false))
}

/// Whether a recorded voice is a man's ("male") or a woman's ("female"),
/// from its middle pitch: reading when there is some (men about 85-155 Hz,
/// women 165-255), else singing (higher for both). None in between.
pub fn sex_of(paths: &Paths, c: &Cipher, voice: &OwnVoice) -> Option<&'static str> {
    const RATE: u32 = 16_000;
    let (samples, man, woman) = match joined(paths, c, voice, &voice.order(LineKind::Speak, SPEAK_ORDER), RATE, 15.0) {
        Ok((s, _)) if s.len() > RATE as usize * 2 => (s, 160.0, 175.0),
        _ => (joined(paths, c, voice, &voice.order(LineKind::Sing, SING_ORDER), RATE, 15.0).ok()?.0, 210.0, 250.0),
    };
    sex_by_pitch(middle_pitch(&samples, RATE)?, man, woman)
}

fn sex_by_pitch(hz: f32, man: f32, woman: f32) -> Option<&'static str> {
    if hz <= man {
        Some("male")
    } else if hz >= woman {
        Some("female")
    } else {
        None
    }
}

/// The middle pitch of the voiced moments (autocorrelation, 80 to 800 Hz).
/// None when too little of it is voiced to tell.
fn middle_pitch(samples: &[f32], rate: u32) -> Option<f32> {
    let frame = rate as usize / 20;
    let loudest = samples.chunks(frame).map(crate::audio::rms).fold(0.0f32, f32::max).max(1e-6);
    let (lo, hi) = (rate as usize / 800, (rate as usize / 80).min(frame - 1));
    let mut pitches: Vec<f32> = samples
        .chunks(frame)
        .filter(|f| f.len() == frame && crate::audio::rms(f) > loudest * 0.2)
        .filter_map(|f| {
            let energy: f32 = f.iter().map(|x| x * x).sum::<f32>().max(1e-9);
            let (lag, peak) = (lo..hi).map(|l| (l, f.iter().zip(&f[l..]).map(|(a, b)| a * b).sum::<f32>() / energy)).fold((0, 0.0f32), |best, x| if x.1 > best.1 { x } else { best });
            (peak > 0.5 && lag > 0).then(|| rate as f32 / lag as f32)
        })
        .collect();
    if pitches.len() < 10 {
        return None;
    }
    pitches.sort_by(|a, b| a.total_cmp(b));
    Some(pitches[pitches.len() / 2])
}

// ---------- recording ----------

struct Recording {
    voice: String,
    line: String,
    capture: Capture,
    samples: Arc<Mutex<Vec<f32>>>,
}

static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

/// A take cleaned up: quiet ends trimmed, brought to a steady level, and
/// what could be better about it. Too short or silent is an error.
fn clean(samples: &[f32], min_s: f32) -> Result<(Vec<f32>, Vec<String>), String> {
    const FRAME: usize = (REC_RATE / 50) as usize;
    let rms: Vec<f32> = samples.chunks(FRAME).map(crate::audio::rms).collect();
    if rms.is_empty() {
        return Err("Nothing was recorded. Check the microphone in Settings › Voice.".into());
    }
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    let (floor, loud) = (at(0.1), at(0.95));
    // Never above half the loud level: a take with no quiet at all (started
    // the moment Record was pressed) is all voice, not nothing.
    let threshold = (floor * 2.5).max(loud * 0.08).max(0.003).min(loud * 0.5);
    let first = rms.iter().position(|r| *r > threshold);
    let last = rms.iter().rposition(|r| *r > threshold);
    let (Some(first), Some(last)) = (first, last) else {
        return Err("I couldn't hear anything. Check the microphone in Settings › Voice.".into());
    };
    let start = first.saturating_sub(10) * FRAME;
    let end = ((last + 16) * FRAME).min(samples.len());
    let mut take = samples[start..end].to_vec();
    let seconds = take.len() as f32 / REC_RATE as f32;
    if seconds < min_s {
        return Err(format!("That was only {seconds:.1} seconds. Say the whole line, then press Stop."));
    }
    let peak = take.iter().fold(0f32, |m, s| m.max(s.abs()));
    let clipped = take.iter().filter(|s| s.abs() >= 0.985).count() as f32 / take.len() as f32;
    let mut notes = Vec::new();
    if clipped > 0.0005 {
        notes.push("loud".to_string());
    } else if peak < 0.06 {
        notes.push("quiet".to_string());
    }
    // Noise shows in the pauses; with hardly any, it can't be told.
    let pauses = rms.iter().filter(|r| **r < threshold).count();
    if pauses * 10 >= rms.len() && 20.0 * (loud / floor.max(1e-5)).log10() < 20.0 {
        notes.push("noisy".to_string());
    }
    // About -20 dBFS where it's loud, never past 0.95.
    let gain = (0.1 / loud.max(1e-4)).min(0.95 / peak.max(1e-4));
    take.iter_mut().for_each(|s| *s *= gain);
    Ok((take, notes))
}

/// What speech recognition heard in a take (None when no speech model is
/// installed or it heard nothing).
async fn heard(state: &Arc<AppState>, take: &[f32]) -> Option<String> {
    if crate::speech::installed(&state.db.lock().unwrap()).is_empty() {
        return None;
    }
    let _hold = crate::speech::hold(state);
    let ep = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.ok()?;
    let s16 = stems::resample(take, REC_RATE, crate::audio::RATE);
    let t = crate::speech::transcribe(&ep, &s16, &crate::speech::Options::default()).await.ok()?;
    Some(t.text.trim().to_string()).filter(|t| !t.is_empty())
}

#[derive(Serialize)]
pub struct OwnVoicesView {
    voices: Vec<VoiceView>,
    /// Lines: id, kind, shortest and longest take.
    lines: Vec<serde_json::Value>,
    /// What narration and singing download the first time (bytes).
    speak_size: u64,
    sing_size: u64,
    /// What making voices from words downloads the first time (0: it can't
    /// here).
    design_size: u64,
    /// Seconds of clear speech a voice from recordings needs, and that make
    /// it sound most like the person.
    speech_enough: f32,
    speech_best: f32,
}

#[derive(Serialize)]
struct VoiceView {
    #[serde(flatten)]
    voice: OwnVoice,
    can_speak: bool,
    can_sing: bool,
}

fn view(state: &AppState) -> Result<OwnVoicesView, String> {
    let c = state.work_cipher()?;
    let voices = list(&state.db.lock().unwrap(), &c).into_iter().map(|v| VoiceView { can_speak: v.can_speak(), can_sing: v.can_sing(), voice: v }).collect();
    let lines = LINES.iter().map(|(id, kind, min, max)| json!({ "id": id, "kind": kind, "min_s": min, "max_s": max })).collect();
    let cat = state.catalog.media.own_voice.as_ref();
    let gpu = state.budget().vram > 0;
    let speak_size = cat.map_or(0, |c| (if gpu { &c.speak.files } else { &c.speak.processor_files }).iter().map(|f| f.size).sum::<u64>());
    let sing_size = cat.map_or(0, |c| c.sing.files.iter().map(|f| f.size).sum());
    let design_size = cat.and_then(|c| c.design.as_ref()).map_or(0, |d| d.files.iter().map(|f| f.size).sum());
    Ok(OwnVoicesView { voices, lines, speak_size, sing_size, design_size, speech_enough: SPEECH_ENOUGH, speech_best: SPEECH_BEST })
}

#[tauri::command]
pub fn own_voices(state: AppStateRef<'_>) -> Result<OwnVoicesView, String> {
    view(&state)
}

/// A new voice. `agreed`: the person says it's their own voice, or that
/// whoever it belongs to said yes.
#[tauri::command]
pub fn own_voice_create(state: AppStateRef<'_>, name: String, agreed: bool) -> Result<OwnVoicesView, String> {
    crate::features::require(&state, crate::features::Feature::Music)?;
    if !agreed {
        return Err("Only record your own voice, or someone who has said yes.".into());
    }
    let name = name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err("Give the voice a name.".into());
    }
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    let now = db::now_ms();
    all.push(OwnVoice { id: super::store::new_id(), name, takes: Vec::new(), created_at: now, agreed_at: now, designed: None });
    store(&conn, &c, &all)?;
    drop(conn);
    state.log("media", "Added a voice to record");
    view(&state)
}

#[tauri::command]
pub fn own_voice_rename(state: AppStateRef<'_>, id: String, name: String) -> Result<OwnVoicesView, String> {
    let name = name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err("Give the voice a name.".into());
    }
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    all.iter_mut().find(|v| v.id == id).ok_or("That voice is no longer here.")?.name = name;
    store(&conn, &c, &all)?;
    drop(conn);
    view(&state)
}

/// Removes a voice and every recording of it.
#[tauri::command]
pub fn own_voice_delete(state: AppStateRef<'_>, id: String) -> Result<OwnVoicesView, String> {
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    all.retain(|v| v.id != id);
    store(&conn, &c, &all)?;
    drop(conn);
    let dir = take_path(&state.paths, &id, "x")?.parent().map(Path::to_path_buf);
    if let Some(d) = dir {
        let _ = std::fs::remove_dir_all(d);
    }
    state.log("media", "Deleted a recorded voice");
    view(&state)
}

/// Starts recording a line. "own_voice:level" events carry the loudness
/// (0..1) and the seconds so far; at the line's longest it stops by itself
/// ("own_voice:full").
#[tauri::command]
pub async fn own_voice_record(app: AppHandle, state: AppStateRef<'_>, voice: String, line: String) -> Result<(), String> {
    crate::features::require(&state, crate::features::Feature::Music)?;
    let (_, _, max_s) = self::line(&line).ok_or("Unknown line.")?;
    let c = state.work_cipher()?;
    if get(&state.db.lock().unwrap(), &c, &voice).is_none() {
        return Err("That voice is no longer here.".into());
    }
    let mut slot = RECORDING.lock().unwrap();
    if slot.is_some() {
        return Err("A recording is already running.".into());
    }
    let mic = crate::speech::voice_settings(&state.db.lock().unwrap()).mic;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
    // Windows' voice processing (echo and noise removal) changes how a voice
    // sounds: it's off for these.
    let capture = Capture::start_at(Source::Mic { device: mic, voice: false }, tx, REC_RATE)?;
    let samples = Arc::new(Mutex::new(Vec::new()));
    let into = samples.clone();
    let (v, l) = (voice.clone(), line.clone());
    tauri::async_runtime::spawn(async move {
        let mut since = 0usize;
        while let Some(chunk) = rx.recv().await {
            let total = {
                let mut all = into.lock().unwrap();
                all.extend_from_slice(&chunk);
                all.len()
            };
            since += chunk.len();
            if since >= (REC_RATE / 10) as usize {
                since = 0;
                let level = (crate::audio::rms(&chunk) * 4.0).min(1.0);
                let _ = app.emit("own_voice:level", json!({ "voice": v, "line": l, "level": level, "seconds": total as f32 / REC_RATE as f32 }));
            }
            if total as f32 / REC_RATE as f32 >= max_s {
                let _ = app.emit("own_voice:full", json!({ "voice": v, "line": l }));
                break;
            }
        }
    });
    *slot = Some(Recording { voice, line, capture, samples });
    Ok(())
}

/// Stops recording and keeps the take (in place of an earlier one of the
/// line). `words`: the line as the window showed it, for when speech
/// recognition isn't installed.
#[tauri::command]
pub async fn own_voice_stop(state: AppStateRef<'_>, words: String) -> Result<OwnVoicesView, String> {
    let rec = RECORDING.lock().unwrap().take().ok_or("Nothing is being recorded.")?;
    let device_error = rec.capture.error();
    drop(rec.capture);
    let samples = std::mem::take(&mut *rec.samples.lock().unwrap());
    if let Some(e) = device_error.filter(|_| samples.is_empty()) {
        return Err(e);
    }
    let (kind, min_s, _) = line(&rec.line).ok_or("Unknown line.")?;
    let (take, notes) = clean(&samples, min_s)?;
    let state = state.inner().clone();
    let words = match kind {
        LineKind::Speak => heard(&state, &take).await.unwrap_or(words),
        LineKind::Sing => words,
    };
    let c = state.work_cipher()?;
    let path = take_path(&state.paths, &rec.voice, &rec.line)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&path, c.seal_bytes(&crate::audio::wav_encode(&take, REC_RATE))).map_err(|e| e.to_string())?;
    {
        let conn = state.db.lock().unwrap();
        let mut all = list(&conn, &c);
        let v = all.iter_mut().find(|v| v.id == rec.voice).ok_or("That voice is no longer here.")?;
        v.takes.retain(|t| t.line != rec.line);
        v.takes.push(Take { line: rec.line.clone(), seconds: take.len() as f32 / REC_RATE as f32, words: words.trim().to_string(), notes, at: db::now_ms(), ..Default::default() });
        store(&conn, &c, &all)?;
    }
    view(&state)
}

/// A voice from takes already recorded (line, 48 kHz samples, words), kept
/// the way recording keeps them: for tests.
#[cfg(test)]
pub fn add_for_test(state: &AppState, name: &str, takes: &[(&str, Vec<f32>, &str)]) -> String {
    let c = state.work_cipher().unwrap();
    let id = super::store::new_id();
    let mut voice = OwnVoice { id: id.clone(), name: name.into(), created_at: db::now_ms(), agreed_at: db::now_ms(), takes: Vec::new(), designed: None };
    for (line, samples, words) in takes {
        let path = take_path(&state.paths, &id, line).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, c.seal_bytes(&crate::audio::wav_encode(samples, REC_RATE))).unwrap();
        // An added recording is a reading (`keep_for_test` and the podcast test).
        let kind = line.starts_with(ADDED).then_some(LineKind::Speak);
        voice.takes.push(Take { line: line.to_string(), seconds: samples.len() as f32 / REC_RATE as f32, words: words.to_string(), at: db::now_ms(), kind, score: 1.0, ..Default::default() });
    }
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    all.push(voice);
    store(&conn, &c, &all).unwrap();
    id
}

/// Stops recording and throws it away.
#[tauri::command]
pub fn own_voice_cancel() {
    RECORDING.lock().unwrap().take();
}

/// A take to listen to: WAV, base64.
#[tauri::command]
pub fn own_voice_take(state: AppStateRef<'_>, voice: String, line: String) -> Result<String, String> {
    use base64::Engine as _;
    let c = state.work_cipher()?;
    let take = read_take(&state.paths, &c, &voice, &line)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(crate::audio::wav_encode(&take, REC_RATE)))
}

#[tauri::command]
pub fn own_voice_take_delete(state: AppStateRef<'_>, voice: String, line: String) -> Result<OwnVoicesView, String> {
    let c = state.work_cipher()?;
    {
        let conn = state.db.lock().unwrap();
        let mut all = list(&conn, &c);
        all.iter_mut().find(|v| v.id == voice).ok_or("That voice is no longer here.")?.takes.retain(|t| t.line != line);
        store(&conn, &c, &all)?;
    }
    let _ = std::fs::remove_file(take_path(&state.paths, &voice, &line)?);
    view(&state)
}

// ---------- recordings the person adds ----------

/// Where the voice is clearest: the `len_s` stretch with the most voiced
/// 50 ms frames (the louder on a tie), as sample bounds, and the share of
/// it that's voice.
fn clearest(samples: &[f32], rate: u32, len_s: f32) -> (usize, usize, f32) {
    let frame = (rate / 20).max(1) as usize;
    let rms: Vec<f32> = samples.chunks(frame).map(crate::audio::rms).collect();
    if rms.is_empty() {
        return (0, 0, 0.0);
    }
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let loud = sorted[((sorted.len() - 1) as f32 * 0.95) as usize];
    let voiced: Vec<bool> = rms.iter().map(|r| *r > (loud * 0.2).max(0.003)).collect();
    let w = ((len_s * 20.0) as usize).clamp(1, rms.len());
    let (mut count, mut sum) = (0usize, 0f32);
    for i in 0..w {
        if voiced[i] {
            count += 1;
            sum += rms[i];
        }
    }
    let (mut best, mut best_start) = ((count, sum), 0);
    for i in w..rms.len() {
        if voiced[i] {
            count += 1;
            sum += rms[i];
        }
        if voiced[i - w] {
            count -= 1;
            sum -= rms[i - w];
        }
        if count > best.0 || (count == best.0 && sum > best.1) {
            best = (count, sum);
            best_start = i + 1 - w;
        }
    }
    (best_start * frame, ((best_start + w) * frame).min(samples.len()), best.0 as f32 / w as f32)
}

/// One file added to a voice: prepared (`prepare_one`), then sealed as a
/// new take. The script's lines stay as they are.
async fn add_one(state: &Arc<AppState>, c: &Cipher, voice_id: &str, path: &Path, kind: LineKind, say: &(dyn Fn(&str) + Sync)) -> Result<(), String> {
    let p = prepare_one(state, path, kind, say).await?;
    store_one(state, c, voice_id, p)
}

/// A recording made ready to keep: its clearest stretch, levelled, what was
/// said in it, and how good it is.
#[derive(Clone)]
struct Prepared {
    take: Vec<f32>,
    words: String,
    notes: Vec<String>,
    score: f32,
    kind: LineKind,
    file: String,
    /// Others talk in the recording too (only whoever talks most was kept).
    several: bool,
}

impl Prepared {
    fn seconds(&self) -> f32 {
        self.take.len() as f32 / REC_RATE as f32
    }

    /// How well it can stand for the voice (0 to 1): how much of it is
    /// voice, less for each problem (quiet, distorted, noisy), less when
    /// it's short.
    fn quality(&self) -> f32 {
        let length = (self.seconds() / 12.0).min(1.0);
        (self.score - 0.15 * self.notes.len() as f32).clamp(0.0, 1.0) * (0.4 + 0.6 * length)
    }
}

/// Seals a prepared recording as a new take of a voice.
fn store_one(state: &AppState, c: &Cipher, voice_id: &str, p: Prepared) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, c);
    let v = all.iter_mut().find(|v| v.id == voice_id).ok_or("That voice is no longer here.")?;
    let n = v.takes.iter().filter_map(|t| t.line.strip_prefix(ADDED)?.parse::<u32>().ok()).max().map_or(1, |n| n + 1);
    let line = format!("{ADDED}{n}");
    let dest = take_path(&state.paths, voice_id, &line)?;
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&dest, c.seal_bytes(&crate::audio::wav_encode(&p.take, REC_RATE))).map_err(|e| e.to_string())?;
    let seconds = p.seconds();
    v.takes.push(Take { line, seconds, words: p.words.trim().to_string(), notes: p.notes, at: db::now_ms(), kind: Some(p.kind), file: p.file, score: p.score });
    store(&conn, c, &all)
}

/// A file decoded, the voice taken out of the music when it's a song (an a
/// cappella stays as sung), its clearest 15 s kept and levelled, and what
/// was said in it heard.
async fn prepare_one(state: &Arc<AppState>, path: &Path, kind: LineKind, say: &(dyn Fn(&str) + Sync)) -> Result<Prepared, String> {
    say("reading");
    let file = path.to_path_buf();
    let (samples, rate, channels) = tokio::task::spawn_blocking(move || super::audio_in::decode(&file)).await.map_err(|e| e.to_string())??;
    let ch = channels.max(1) as usize;
    let seconds = samples.len() as f32 / ch as f32 / rate.max(1) as f32;
    if seconds < 3.0 {
        return Err("It's shorter than 3 seconds.".into());
    }
    // Talking can be a whole podcast episode; a song is shorter.
    let longest = if kind == LineKind::Speak { 90.0 } else { 20.0 };
    if seconds > longest * 60.0 {
        return Err(format!("It's longer than {longest:.0} minutes."));
    }
    if kind == LineKind::Speak {
        let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect();
        drop(samples);
        return prepare_speech(state, mono, rate, say, path).await;
    }
    let left: Vec<f32> = samples.chunks(ch).map(|f| f[0]).collect();
    let right: Vec<f32> = samples.chunks(ch).map(|f| f[if ch > 1 { 1 } else { 0 }]).collect();
    drop(samples);
    let song = stems::Stereo { rate, ch: [left, right] }.resampled(stems::RATE);
    let voice = if kind == LineKind::Sing {
        say("separating");
        let never = AtomicBool::new(false);
        let (dll, model, card) = super::separator(state, &never, &|_, _| {}).await?;
        let threads = state.limits().threads;
        let mix = song.mono();
        let vocals = tokio::task::spawn_blocking(move || {
            let r = stems::separate(&dll, &model, &song, threads, &[3], card, &AtomicBool::new(false)).map(|(parts, _)| parts[0].mono());
            stems::unload();
            r
        })
        .await
        .map_err(|e| e.to_string())??;
        // An a cappella: the voice taken out is nearly all of it, so it's kept
        // as sung (taking it out leaves traces).
        if crate::audio::rms(&vocals) > 0.9 * crate::audio::rms(&mix) {
            mix
        } else {
            vocals
        }
    } else {
        song.mono()
    };
    say("choosing");
    let at48 = stems::resample(&voice, stems::RATE, REC_RATE);
    drop(voice);
    let (a, b, score) = clearest(&at48, REC_RATE, ADDED_S);
    if score < 0.25 {
        return Err("There's too little voice in it.".into());
    }
    let (take, notes) = clean(&at48[a..b], 3.0)?;
    let words = match kind {
        LineKind::Speak => heard(state, &take).await.unwrap_or_default(),
        LineKind::Sing => String::new(),
    };
    let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    Ok(Prepared { take, words, notes, score, kind, file, several: false })
}

/// A recording of the person talking, made ready: in a long one (a
/// podcast), only whoever talks most (by voice prints: not the guest, not
/// the intro music); its clearest 15 s, with any music under it taken out;
/// and its words heard, which narration needs to sound like the person and
/// not only roughly like them.
async fn prepare_speech(state: &Arc<AppState>, mono: Vec<f32>, rate: u32, say: &(dyn Fn(&str) + Sync), path: &Path) -> Result<Prepared, String> {
    let mut several = false;
    let mut voice = mono;
    if voice.len() as f32 / rate as f32 > 30.0 {
        match main_speaker(state, &voice, rate, say).await {
            Some((kept, more)) => {
                voice = kept;
                several = more;
            }
            None => crate::applog::line("media", "Couldn't tell the speakers apart in an added recording; using all of it"),
        }
    }
    say("choosing");
    let at48 = stems::resample(&voice, rate, REC_RATE);
    drop(voice);
    let (a, b, score) = clearest(&at48, REC_RATE, ADDED_S);
    if score < 0.25 {
        return Err("There's too little voice in it.".into());
    }
    let take = without_music(state, &at48[a..b], say).await;
    let (take, notes) = clean(&take, 3.0)?;
    say("hearing");
    let words = words_of(state, &take, say).await.unwrap_or_default();
    let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    Ok(Prepared { take, words, notes, score, kind: LineKind::Speak, file, several })
}

/// Whoever talks most in a long recording: 3-second stretches with voice
/// in them, up to 160 across it, grouped by voice print; the biggest group's
/// stretches joined in order. Also whether others talk in it. None when it
/// can't tell (the speaker model can't be had, or there's too little voice).
async fn main_speaker(state: &Arc<AppState>, mono: &[f32], rate: u32, say: &(dyn Fn(&str) + Sync)) -> Option<(Vec<f32>, bool)> {
    if !crate::diarize::available(state) {
        say("getting_speakers");
        crate::diarize::install(state, &AtomicBool::new(false), &|_, _, _| {}).await.map_err(|e| crate::applog::line("media", &format!("The speaker model didn't install: {e}"))).ok()?;
    }
    say("speakers");
    const WIN: usize = 16_000 * 3;
    let at16 = stems::resample(mono, rate, 16_000);
    let loud = at16.chunks(WIN).map(crate::audio::rms).fold(0f32, f32::max);
    let mut starts: Vec<usize> = (0..at16.len().saturating_sub(WIN)).step_by(WIN).filter(|&s| crate::audio::rms(&at16[s..s + WIN]) > (loud * 0.15).max(0.01)).collect();
    if starts.len() > 160 {
        let step = starts.len() as f32 / 160.0;
        starts = (0..160).map(|i| starts[(i as f32 * step) as usize]).collect();
    }
    if starts.len() < 4 {
        return None;
    }
    let st = state.clone();
    let at = starts.clone();
    let prints: Vec<(usize, Vec<f32>)> = tokio::task::spawn_blocking(move || at.iter().filter_map(|&s| crate::diarize::embed(&st, &at16[s..s + WIN]).ok().map(|p| (s, p))).collect()).await.ok()?;
    crate::diarize::unload();
    if prints.len() < 4 {
        return None;
    }
    let labels = crate::diarize::regroup(&prints.iter().map(|p| p.1.clone()).collect::<Vec<_>>());
    let mut count: HashMap<u32, usize> = HashMap::new();
    for l in &labels {
        *count.entry(*l).or_default() += 1;
    }
    let (main, n) = count.iter().max_by_key(|(_, n)| **n).map(|(l, n)| (*l, *n))?;
    // Others talk when they have a real share of it, not a stray stretch or two.
    let several = count.iter().any(|(l, c)| *l != main && *c as f32 >= (prints.len() as f32 * 0.1).max(2.0));
    let scale = rate as f64 / 16_000.0;
    let mut kept = Vec::new();
    for ((s, _), l) in prints.iter().zip(&labels) {
        if *l == main {
            let from = (*s as f64 * scale) as usize;
            let to = (((*s + WIN) as f64) * scale) as usize;
            kept.extend_from_slice(&mono[from.min(mono.len())..to.min(mono.len())]);
        }
    }
    crate::applog::line("media", &format!("An added recording: {} of {} stretches are its main speaker{}", n, prints.len(), if several { "; others talk in it too" } else { "" }));
    Some((kept, several))
}

/// Any music under the voice taken out (the part separator, on these few
/// seconds only); as it was when there's none, or no separator.
async fn without_music(state: &Arc<AppState>, take: &[f32], say: &(dyn Fn(&str) + Sync)) -> Vec<f32> {
    let never = AtomicBool::new(false);
    let Ok((dll, model, card)) = super::separator(state, &never, &|_, _| {}).await else { return take.to_vec() };
    say("separating");
    let mono = stems::resample(take, REC_RATE, stems::RATE);
    let stereo = stems::Stereo { rate: stems::RATE, ch: [mono.clone(), mono.clone()] };
    let threads = state.limits().threads;
    let vocals = tokio::task::spawn_blocking(move || {
        let r = stems::separate(&dll, &model, &stereo, threads, &[3], card, &AtomicBool::new(false)).map(|(parts, _)| parts[0].mono());
        stems::unload();
        r
    })
    .await;
    match vocals {
        // Mostly voice already: kept as it was (taking it out leaves traces).
        Ok(Ok(v)) if crate::audio::rms(&v) < 0.9 * crate::audio::rms(&mono) => {
            crate::applog::line("media", "Took music out from under an added recording's voice");
            stems::resample(&v, stems::RATE, REC_RATE)
        }
        _ => take.to_vec(),
    }
}

/// The words said in a reading, heard by speech recognition: a small model
/// is fetched the first time when none is installed (without them, a voice
/// is copied only roughly).
async fn words_of(state: &Arc<AppState>, take: &[f32], say: &(dyn Fn(&str) + Sync)) -> Option<String> {
    if crate::speech::installed(&state.db.lock().unwrap()).is_empty() {
        let spec = state.catalog.speech.models.iter().find(|m| m.id == "whisper-base").or_else(|| state.catalog.speech.models.first())?.clone();
        say("getting_words");
        if let Err(e) = crate::speech::install(state, &spec, &AtomicBool::new(false), &|_, _, _| {}).await {
            crate::applog::line("media", &format!("Speech recognition didn't install for a voice ({e}); it's copied from the sound alone"));
            return None;
        }
    }
    heard(state, take).await
}

// ---------- a voice from recordings alone ----------

/// Recordings checked for a new voice, waiting for "Create voice": by the
/// token the window got for each.
static CHECKED: Mutex<Vec<(String, Prepared)>> = Mutex::new(Vec::new());

/// What a check found in one recording.
#[derive(Serialize)]
pub struct Checked {
    /// To keep it with `own_voice_create_from` (None: it can't be used).
    token: Option<String>,
    file: String,
    kind: LineKind,
    /// Seconds kept: its clearest stretch, up to 15.
    seconds: f32,
    /// How much of what's kept is voice (0 to 1).
    clear: f32,
    /// How well it can stand for the voice (0 to 1).
    quality: f32,
    /// "quiet", "loud" (it distorted), "noisy".
    notes: Vec<String>,
    /// What speech recognition heard (speaking only).
    words: String,
    /// Why it can't be used.
    problem: Option<String>,
    /// Others talk in it too: only whoever talks most was kept.
    several: bool,
}

/// Seconds of clear speech that make a voice good enough for narration, and
/// that make it sound most like the person (narration clones from up to 15 s).
pub const SPEECH_ENOUGH: f32 = ENOUGH_SPEECH_S;
pub const SPEECH_BEST: f32 = 15.0;

/// Reads recordings the person has, for a voice made from them alone: each
/// gets a rating, and nothing is kept until they create the voice.
/// "own_voice:adding" events say which file it's on.
#[tauri::command]
pub async fn own_voice_check(app: AppHandle, state: AppStateRef<'_>, paths: Vec<String>, singing: bool) -> Result<Vec<Checked>, String> {
    crate::features::require(&state, crate::features::Feature::Music)?;
    let state = state.inner().clone();
    let kind = if singing { LineKind::Sing } else { LineKind::Speak };
    let mut out = Vec::new();
    for (i, path) in paths.iter().take(MAX_ADDED).enumerate() {
        let name = Path::new(path.as_str()).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let say = |step: &str| {
            let _ = app.emit("own_voice:adding", json!({ "voice": "", "file": name, "step": step, "n": i + 1, "of": paths.len().min(MAX_ADDED) }));
        };
        match prepare_one(&state, Path::new(path.as_str()), kind, &say).await {
            Ok(p) => {
                let token = uuid::Uuid::new_v4().simple().to_string();
                out.push(Checked { token: Some(token.clone()), file: p.file.clone(), kind, seconds: p.seconds(), clear: p.score, quality: p.quality(), notes: p.notes.clone(), words: p.words.clone(), problem: None, several: p.several });
                let mut held = CHECKED.lock().unwrap();
                held.push((token, p));
                // Only the latest few dozen wait.
                let extra = held.len().saturating_sub(48);
                held.drain(..extra);
            }
            Err(e) => out.push(Checked { token: None, file: name.clone(), kind, seconds: 0.0, clear: 0.0, quality: 0.0, notes: Vec::new(), words: String::new(), problem: Some(e), several: false }),
        }
    }
    Ok(out)
}

/// A recording of the person talking made ready, as a check would (for
/// tests): what was kept (48 kHz), its words, and whether others talk in it.
#[cfg(test)]
pub async fn prepare_for_test(state: &Arc<AppState>, path: &Path) -> Result<(Vec<f32>, String, bool), String> {
    let p = prepare_one(state, path, LineKind::Speak, &|step| println!("  {step}")).await?;
    Ok((p.take, p.words, p.several))
}

/// What was kept of a checked recording, to listen to: WAV, base64.
#[tauri::command]
pub fn own_voice_checked_take(token: String) -> Result<String, String> {
    use base64::Engine as _;
    let held = CHECKED.lock().unwrap();
    let p = held.iter().find(|(k, _)| *k == token).map(|(_, p)| p).ok_or("Check the recording again.")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(crate::audio::wav_encode(&p.take, REC_RATE)))
}

/// A new voice from recordings checked with `own_voice_check` (their
/// tokens). `agreed`: it's the person's own voice, or its owner said yes.
#[tauri::command]
pub fn own_voice_create_from(state: AppStateRef<'_>, name: String, agreed: bool, tokens: Vec<String>) -> Result<OwnVoicesView, String> {
    crate::features::require(&state, crate::features::Feature::Music)?;
    if !agreed {
        return Err("Only use your own voice, or someone who has said yes.".into());
    }
    let name = name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err("Give the voice a name.".into());
    }
    let picked: Vec<Prepared> = {
        let held = CHECKED.lock().unwrap();
        tokens.iter().filter_map(|t| held.iter().find(|(k, _)| k == t).map(|(_, p)| p.clone())).collect()
    };
    let speech: f32 = picked.iter().filter(|p| p.kind == LineKind::Speak).map(|p| p.seconds() * p.score).sum();
    let singing = picked.iter().any(|p| p.kind == LineKind::Sing);
    if speech < SPEECH_ENOUGH && !singing {
        return Err(format!("There's about {speech:.0} s of clear speech: add recordings until there's at least {SPEECH_ENOUGH:.0} s."));
    }
    let c = state.work_cipher()?;
    let id = super::store::new_id();
    {
        let conn = state.db.lock().unwrap();
        let mut all = list(&conn, &c);
        let now = db::now_ms();
        all.push(OwnVoice { id: id.clone(), name, takes: Vec::new(), created_at: now, agreed_at: now, designed: None });
        store(&conn, &c, &all)?;
    }
    for p in picked {
        store_one(&state, &c, &id, p)?;
    }
    CHECKED.lock().unwrap().retain(|(k, _)| !tokens.contains(k));
    state.log("media", "Made a voice from recordings the person added");
    view(&state)
}

#[derive(Serialize)]
pub struct AddedView {
    #[serde(flatten)]
    view: OwnVoicesView,
    /// Files that couldn't be used, and why ("name: reason").
    problems: Vec<String>,
}

/// Adds recordings the person already has to a voice: songs they sang
/// (`singing`) or recordings of them speaking. "own_voice:adding" events say
/// which file it's on and what it's doing.
#[tauri::command]
pub async fn own_voice_upload(app: AppHandle, state: AppStateRef<'_>, voice: String, paths: Vec<String>, singing: bool) -> Result<AddedView, String> {
    crate::features::require(&state, crate::features::Feature::Music)?;
    let state = state.inner().clone();
    let c = state.work_cipher()?;
    let v = get(&state.db.lock().unwrap(), &c, &voice).ok_or("That voice is no longer here.")?;
    let room = MAX_ADDED.saturating_sub(v.takes.iter().filter(|t| t.added()).count());
    let kind = if singing { LineKind::Sing } else { LineKind::Speak };
    let mut problems = Vec::new();
    let todo: Vec<&String> = paths.iter().take(room).collect();
    for (i, path) in todo.iter().enumerate() {
        let name = Path::new(path.as_str()).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let say = |step: &str| {
            let _ = app.emit("own_voice:adding", json!({ "voice": voice, "file": name, "step": step, "n": i + 1, "of": todo.len() }));
        };
        if let Err(e) = add_one(&state, &c, &voice, Path::new(path.as_str()), kind, &say).await {
            problems.push(format!("{name}: {e}"));
        }
    }
    if paths.len() > room {
        problems.push(format!("A voice keeps up to {MAX_ADDED} added recordings; remove some to add more."));
    }
    state.log("media", &format!("Added {} recording(s) to a voice", todo.len().saturating_sub(problems.len())));
    Ok(AddedView { view: view(&state)?, problems })
}

// ---------- narration in the voice ----------

/// Qwen3-TTS's name for a language (None: it works it out).
fn tts_language(code: Option<&str>) -> &'static str {
    match code.map(|c| c.split(['-', '_']).next().unwrap_or("").to_lowercase()).as_deref() {
        Some("en") => "English",
        Some("zh") => "Chinese",
        Some("ja") => "Japanese",
        Some("ko") => "Korean",
        Some("de") => "German",
        Some("fr") => "French",
        Some("ru") => "Russian",
        Some("pt") => "Portuguese",
        Some("es") => "Spanish",
        Some("it") => "Italian",
        _ => "auto",
    }
}

/// Text in lines of whole sentences, each at most ~300 characters: the
/// engine speaks each line on its own, with the person's voice each time.
fn lines_of(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for paragraph in text.split('\n').map(str::trim).filter(|p| !p.is_empty()) {
        let mut sentence = String::new();
        let flush = |s: &mut String, cur: &mut String, out: &mut Vec<String>| {
            let s2 = s.trim().to_string();
            s.clear();
            if s2.is_empty() {
                return;
            }
            if !cur.is_empty() && cur.len() + s2.len() > 300 {
                out.push(std::mem::take(cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(&s2);
        };
        for ch in paragraph.chars() {
            sentence.push(ch);
            if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | ';') {
                flush(&mut sentence, &mut cur, &mut out);
            }
        }
        flush(&mut sentence, &mut cur, &mut out);
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    out
}

/// Each line's WAV from the engine's output (`--stream-by-line -o -`): one
/// header per line, each 44 bytes with no length (0x7fffffff), 16-bit mono.
fn split_streamed(bytes: &[u8]) -> Vec<Vec<f32>> {
    const MARK: &[u8] = b"RIFF\xff\xff\xff\x7fWAVE";
    let mut starts: Vec<usize> = bytes.windows(MARK.len()).enumerate().filter(|(_, w)| *w == MARK).map(|(i, _)| i).collect();
    starts.push(bytes.len());
    starts
        .windows(2)
        .map(|w| bytes[(w[0] + 44).min(w[1])..w[1]].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect())
        .collect()
}

/// The narration engine, downloaded the first time: the graphics-card build
/// where there's a card.
async fn voice_engine(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<PathBuf, String> {
    let spec = state.catalog.media.own_voice.as_ref().map(|c| c.speak.clone()).ok_or("Speaking in your voice isn't available on this system yet.")?;
    let gpu = state.budget().vram > 0 && cfg!(all(windows, target_arch = "x86_64"));
    let getting = "Getting the voice engine (once)";
    if let Some(asset) = spec.engine.assets.get("vulkan-x64").filter(|_| gpu) {
        let dir = format!("qwentts-{}-vulkan-x64", spec.engine.build);
        match engine::ensure_unpacked(&state.paths, &dir, asset, TTS_EXE, cancel, |r, t| progress(getting, r as f64 / t.max(1) as f64)).await {
            Ok(p) => return Ok(p),
            Err(e) if e == download::CANCELLED => return Err(e),
            Err(e) => state.log("media", &format!("The graphics-card voice engine didn't download ({e}); using the processor one")),
        }
    }
    let asset = spec.engine.assets.get(engine::cpu_key()).ok_or("Speaking in your voice isn't available on this system yet.")?;
    let dir = format!("qwentts-{}-{}", spec.engine.build, engine::cpu_key());
    engine::ensure_unpacked(&state.paths, &dir, asset, TTS_EXE, cancel, |r, t| progress(getting, r as f64 / t.max(1) as f64)).await
}

/// The narration engine and model, downloaded the first time: the
/// graphics-card build and the bigger model where there's a card.
async fn speak_parts(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let spec = state.catalog.media.own_voice.as_ref().map(|c| c.speak.clone()).ok_or("Speaking in your voice isn't available on this system yet.")?;
    let exe = voice_engine(state, cancel, progress).await?;
    let on_card = exe.to_string_lossy().contains("vulkan");
    let files = if on_card { &spec.files } else { &spec.processor_files };
    Ok((exe, fetch_all(state, files, "own-voice", "Getting the voice model (once", cancel, progress).await?))
}

/// Whether reading in a recorded voice can start without downloading
/// anything (Check this PC tries it only then).
pub fn ready_here(state: &AppState) -> bool {
    let Some(spec) = state.catalog.media.own_voice.as_ref().map(|c| &c.speak) else { return false };
    let base = state.paths.models.join("media").join("own-voice");
    let has = |files: &[MediaFile]| files.iter().all(|f| base.join(&f.file).metadata().is_ok_and(|m| m.len() == f.size));
    let engine_here = ["vulkan-x64", engine::cpu_key()].iter().any(|k| state.paths.engines.join(format!("qwentts-{}-{}", spec.engine.build, k)).is_dir());
    engine_here && (has(&spec.files) || has(&spec.processor_files))
}

/// The engine and the model that makes voices from words, downloaded the
/// first time.
async fn design_parts(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let spec = state.catalog.media.own_voice.as_ref().and_then(|c| c.design.clone()).ok_or("Making a voice from words isn't available on this system yet.")?;
    let exe = voice_engine(state, cancel, progress).await?;
    Ok((exe, fetch_all(state, &spec.files, "own-voice", "Getting the voice maker (once", cancel, progress).await?))
}

/// Model files into models/media/<dir>, downloaded the first time.
async fn fetch_all(state: &Arc<AppState>, files: &[MediaFile], dir: &str, getting: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<PathBuf>, String> {
    let base = state.paths.models.join("media").join(dir);
    let missing: u64 = files.iter().filter(|f| !base.join(&f.file).metadata().is_ok_and(|m| m.len() == f.size)).map(|f| f.size).sum();
    let label = format!("{getting}, {})", crate::size_label(missing));
    let mut done = 0u64;
    let mut out = Vec::new();
    for f in files {
        let dest = base.join(&f.file);
        if !dest.metadata().is_ok_and(|m| m.len() == f.size) {
            state.log("network", &format!("Downloading {} ({}) from {}", f.file, crate::size_label(f.size), crate::host_of(&f.url)));
            let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
            let before = done;
            download::fetch_verified(&client, &f.url, &dest, f.size, &f.sha256, cancel, |r, _| progress(&label, (before + r) as f64 / missing.max(1) as f64)).await?;
            done += f.size;
        }
        out.push(dest);
    }
    Ok(out)
}

/// Reads `text` aloud in a recorded voice: 24 kHz samples.
pub async fn speak(state: &Arc<AppState>, voice_id: &str, text: &str, language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(Vec<f32>, u32), String> {
    let c = state.work_cipher()?;
    hear_missing_words(state, &c, voice_id, &|_| progress("Hearing the words of your recordings (once)", 0.05)).await;
    let voice = get(&state.db.lock().unwrap(), &c, voice_id).ok_or("That voice is no longer here.")?;
    let (reference, words) = speech_reference(&state.paths, &c, &voice)?;
    let lines = lines_of(text);
    if lines.is_empty() {
        return Err("Type the text to read aloud.".into());
    }
    let (exe, files) = speak_parts(state, cancel, progress).await?;
    progress("Reading in your voice", 0.1);
    // A card that's nearly full (a game, a big model) makes the engine crawl
    // (65 s for a sentence): the same engine and model then run on the
    // processor (~10 s).
    let free = card_free_now(state);
    let device = (exe.to_string_lossy().contains("vulkan") && free.is_some_and(|f| f < 3_500 << 20)).then_some("CPU");
    if device.is_some() {
        crate::applog::line("media", &format!("Reading in a recorded voice on the processor: the graphics card has {} free", crate::size_label(free.unwrap_or(0))));
    }
    read_with(&exe, &files, &reference, &words, &lines, language, device, cancel).await
}

/// What the graphics card has free now, from Windows (None when it can't
/// tell).
fn card_free_now(state: &AppState) -> Option<u64> {
    let total = state.hardware.read().unwrap().best_gpu().map_or(0, |g| g.vram_bytes);
    let used = crate::perf::card_memory_used(crate::hardware::dedicated().and_then(|c| c.luid))?;
    (total > 0).then(|| total.saturating_sub(used))
}

/// The longest a line may take read slowly (8 characters a second), and 3 s
/// more.
fn longest_s(line: &str) -> f32 {
    line.chars().count() as f32 / 8.0 + 3.0
}

/// Whose voice the engine reads in.
#[derive(Clone, Copy)]
enum How<'a> {
    /// The reference in `work` (`ref.wav`; with `with_words`, also `ref.txt`,
    /// its words: in-context cloning, which keeps the way the person speaks).
    Clone { work: &'a Path, with_words: bool },
    /// A voice made from this description (the VoiceDesign model).
    Design(&'a str),
}

/// The engine making `lines` in a voice: one reading per line. Each is
/// capped near its line's length (the engine's own cap is ~170 s).
async fn run_engine(exe: &Path, files: &[PathBuf], how: How<'_>, lines: &[String], language: Option<&str>, device: Option<&str>, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    // Qwen3-TTS makes 12.5 frames a second.
    let max_new = lines.iter().map(|l| longest_s(l)).fold(0f32, f32::max) * 12.5;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.current_dir(exe.parent().unwrap_or(Path::new("."))).arg("--model").arg(&files[0]).arg("--codec").arg(&files[1]);
    match how {
        How::Clone { work, with_words } => {
            cmd.arg("--ref-wav").arg(work.join("ref.wav"));
            if with_words {
                cmd.arg("--ref-text").arg(work.join("ref.txt"));
            }
        }
        How::Design(description) => {
            cmd.arg("--instruct").arg(description);
        }
    }
    if let Some(d) = device {
        cmd.args(["--device", d]);
    }
    cmd.args(["--lang", tts_language(language), "--max-new", &(max_new.ceil() as u32).to_string(), "--stream-by-line", "-o", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| format!("The voice engine didn't start: {e}"))?;
    let input = lines.join("\n") + "\n";
    let mut stdin = child.stdin.take().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(input.as_bytes()).await;
    });
    let out = child.wait_with_output();
    tokio::pin!(out);
    let out = loop {
        tokio::select! {
            o = &mut out => break o.map_err(|e| e.to_string())?,
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(download::CANCELLED.to_string());
                }
            }
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        crate::applog::line("media", &format!("The voice engine failed: {}", err.lines().rev().take(4).collect::<Vec<_>>().join(" | ")));
        return Err("The voice engine stopped with an error.".into());
    }
    Ok(split_streamed(&out.stdout))
}

/// Whether any reading came out as sound (not silence all through, nor
/// samples that aren't numbers).
fn heard_any(parts: &[Vec<f32>]) -> bool {
    parts.iter().any(|p| !p.is_empty() && p.iter().all(|s| s.is_finite()) && crate::audio::rms(p) > 0.003)
}

/// `lines` read in the voice of `reference` (24 kHz) whose words are
/// `words`. A line that runs on far past its length (the engine lost its
/// end, as when the words don't match the recording) is read again from the
/// voice alone, without the words. Readings that come out silent on the
/// graphics card (some cards' drivers give silence or broken samples
/// instead of failing) are read again on the processor; silence there too
/// is an error, never saved as a reading.
#[allow(clippy::too_many_arguments)]
async fn read_with(exe: &Path, files: &[PathBuf], reference: &[f32], words: &str, lines: &[String], language: Option<&str>, device: Option<&str>, cancel: &AtomicBool) -> Result<(Vec<f32>, u32), String> {
    let parts = read_lines(exe, files, reference, words, lines, language, device, cancel).await?;
    let gap = vec![0f32; (TTS_RATE as f32 * 0.35) as usize];
    let mut all = Vec::new();
    for p in parts.into_iter().filter(|p| !p.is_empty()) {
        if !all.is_empty() {
            all.extend_from_slice(&gap);
        }
        all.extend(p);
    }
    if all.is_empty() {
        return Err("The voice engine made no sound.".into());
    }
    Ok((all, TTS_RATE))
}

/// Each of `lines` read in the voice of `reference` (24 kHz), checked as
/// `read_with` says: one reading per line (empty where the engine made
/// none), each kept to its line's length.
#[allow(clippy::too_many_arguments)]
async fn read_lines(exe: &Path, files: &[PathBuf], reference: &[f32], words: &str, lines: &[String], language: Option<&str>, device: Option<&str>, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    // The recording goes to the engine as a file for the run only.
    let work = std::env::temp_dir().join(format!("sulcusai-voice-{}", super::store::new_id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = async {
        std::fs::write(work.join("ref.wav"), crate::audio::wav_encode(reference, TTS_RATE)).map_err(|e| e.to_string())?;
        std::fs::write(work.join("ref.txt"), words).map_err(|e| e.to_string())?;
        let cloned = How::Clone { work: &work, with_words: !words.trim().is_empty() };
        let mut parts = run_engine(exe, files, cloned, lines, language, device, cancel).await?;
        let ran_on = |line: &str, p: &[f32]| p.len() as f32 / TTS_RATE as f32 > longest_s(line) - 0.5;
        let again: Vec<usize> = (0..lines.len().min(parts.len())).filter(|i| ran_on(&lines[*i], &parts[*i])).collect();
        if !again.is_empty() {
            crate::applog::line("media", &format!("{} of {} lines ran on in the recorded voice; reading them again from the voice alone", again.len(), lines.len()));
            let retry: Vec<String> = again.iter().map(|i| lines[*i].clone()).collect();
            let redone = run_engine(exe, files, How::Clone { work: &work, with_words: false }, &retry, language, device, cancel).await?;
            for (i, p) in again.iter().zip(redone) {
                if !ran_on(&lines[*i], &p) || p.len() < parts[*i].len() {
                    parts[*i] = p;
                }
            }
        }
        if !heard_any(&parts) && device != Some("CPU") && exe.to_string_lossy().contains("vulkan") {
            crate::applog::line("media", "The graphics card's voice engine made only silence; reading it again on the processor");
            parts = run_engine(exe, files, cloned, lines, language, Some("CPU"), cancel).await?;
        }
        if !heard_any(&parts) {
            crate::applog::line("media", &format!("The voice engine made only silence for {} line(s)", lines.len()));
            return Err("The voice engine made only silence. Try again; if it keeps happening, record your voice's lines again in Studio › Your voice.".into());
        }
        // Samples that aren't numbers in a line that otherwise worked become silence.
        parts.iter_mut().flatten().filter(|s| !s.is_finite()).for_each(|s| *s = 0.0);
        parts.resize(lines.len(), Vec::new());
        for (p, line) in parts.iter_mut().zip(lines) {
            // Still running on: kept to its line's length.
            p.truncate((longest_s(line) * TTS_RATE as f32) as usize);
        }
        Ok(parts)
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Several passages read in a recorded (or made) voice in one go, so the
/// model loads once: one reading per passage at 24 kHz, its sentences a
/// short pause apart. For podcasts and readings with several voices.
pub async fn speak_each(state: &Arc<AppState>, voice_id: &str, passages: &[String], language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(Vec<Vec<f32>>, u32), String> {
    let c = state.work_cipher()?;
    hear_missing_words(state, &c, voice_id, &|_| progress("Hearing the words of your recordings (once)", 0.05)).await;
    let voice = get(&state.db.lock().unwrap(), &c, voice_id).ok_or("That voice is no longer here.")?;
    let (reference, words) = speech_reference(&state.paths, &c, &voice)?;
    speak_each_like(state, &reference, &words, passages, language, cancel, progress).await
}

/// Whether voices read in a recorded voice can speak `language` (Qwen3-TTS
/// knows ten languages).
pub fn speaks(language: &str) -> bool {
    tts_language(Some(language)) != "auto"
}

/// `passages` read as `speak_each` does, in the voice of `reference` (24
/// kHz, a few seconds of someone speaking) whose words are `words` (empty
/// when they aren't known): a voice heard in a video, for dubbing.
#[allow(clippy::too_many_arguments)]
pub async fn speak_each_like(state: &Arc<AppState>, reference: &[f32], words: &str, passages: &[String], language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(Vec<Vec<f32>>, u32), String> {
    // Every passage's sentences in one list, and which passage each is from.
    let mut lines = Vec::new();
    let mut owner = Vec::new();
    for (i, p) in passages.iter().enumerate() {
        for l in lines_of(p) {
            lines.push(l);
            owner.push(i);
        }
    }
    if lines.is_empty() {
        return Ok((vec![Vec::new(); passages.len()], TTS_RATE));
    }
    let (exe, files) = speak_parts(state, cancel, progress).await?;
    let free = card_free_now(state);
    let device = (exe.to_string_lossy().contains("vulkan") && free.is_some_and(|f| f < 3_500 << 20)).then_some("CPU");
    let parts = read_lines(&exe, &files, reference, words, &lines, language, device, cancel).await?;
    let gap = vec![0f32; (TTS_RATE as f32 * 0.3) as usize];
    let mut out = vec![Vec::new(); passages.len()];
    for (p, i) in parts.into_iter().zip(owner) {
        if p.is_empty() {
            continue;
        }
        if !out[i].is_empty() {
            out[i].extend_from_slice(&gap);
        }
        out[i].extend(p);
    }
    Ok((out, TTS_RATE))
}

// ---------- voices made from a description ----------

/// What a made voice reads for its sample (~12 s, kept as its reference):
/// language code and words.
const SAMPLES: &[(&str, &str)] = &[
    ("en", "Hello, and thank you for listening. Today I want to tell you about a small town by the sea, where the mornings are quiet, the bread is always warm, and everybody knows your name."),
    ("de", "Hallo und danke fürs Zuhören. Heute erzähle ich Ihnen von einer kleinen Stadt am Meer, wo die Morgen ruhig sind, das Brot immer warm ist und jeder Ihren Namen kennt."),
    ("es", "Hola, y gracias por escuchar. Hoy quiero hablarles de un pequeño pueblo junto al mar, donde las mañanas son tranquilas, el pan siempre está caliente y todos conocen tu nombre."),
    ("fr", "Bonjour, et merci de votre écoute. Aujourd'hui, je veux vous parler d'une petite ville au bord de la mer, où les matins sont calmes, le pain toujours chaud, et où tout le monde connaît votre nom."),
    ("it", "Ciao, e grazie per l'ascolto. Oggi voglio parlarvi di una piccola città sul mare, dove le mattine sono tranquille, il pane è sempre caldo e tutti conoscono il tuo nome."),
    ("pt", "Olá, e obrigado por ouvir. Hoje quero falar de uma pequena cidade à beira-mar, onde as manhãs são tranquilas, o pão está sempre quente e todos sabem o seu nome."),
    ("ru", "Здравствуйте и спасибо, что слушаете. Сегодня я расскажу вам о маленьком городке у моря, где утро всегда тихое, хлеб всегда тёплый, а все знают ваше имя."),
    ("ja", "こんにちは、聞いてくださってありがとうございます。今日は、海辺の小さな町のお話をします。朝は静かで、パンはいつも温かく、みんながあなたの名前を知っています。"),
    ("ko", "안녕하세요, 들어 주셔서 감사합니다. 오늘은 바닷가의 작은 마을 이야기를 들려 드릴게요. 아침은 조용하고, 빵은 늘 따뜻하며, 모두가 당신의 이름을 알고 있어요."),
    ("zh", "你好，谢谢你的收听。今天我想给你讲一个海边小镇的故事。那里的早晨很安静，面包总是热的，每个人都知道你的名字。"),
];

/// The sample's words in `language` (English when it isn't one of them).
fn sample_words(language: Option<&str>) -> &'static str {
    let code = language.map(|c| c.split(['-', '_']).next().unwrap_or("").to_lowercase()).unwrap_or_default();
    SAMPLES.iter().find(|(c, _)| *c == code).unwrap_or(&SAMPLES[0]).1
}

/// Readings made from the last description, waiting to be kept.
struct Made {
    description: String,
    language: Option<String>,
    takes: Vec<Vec<f32>>,
}

static MADE: Mutex<Option<Made>> = Mutex::new(None);
static DESIGN_CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
pub struct MadeSample {
    /// WAV, base64.
    wav: String,
    seconds: f32,
}

/// Makes three voices from a description, each reading the same sample, to
/// choose from. "own_voice:designing" events say what it's doing.
#[tauri::command]
pub async fn own_voice_design(app: AppHandle, state: AppStateRef<'_>, description: String, language: Option<String>) -> Result<Vec<MadeSample>, String> {
    use base64::Engine as _;
    crate::features::require(&state, crate::features::Feature::Music)?;
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(400).collect::<String>();
    if description.chars().count() < 4 {
        return Err("Describe the voice: who it sounds like (age, man or woman), its pitch, pace and mood.".into());
    }
    let state = state.inner().clone();
    DESIGN_CANCEL.store(false, Ordering::Relaxed);
    let say = |stage: &str, fraction: f64| {
        let _ = app.emit("own_voice:designing", json!({ "stage": stage, "fraction": fraction }));
    };
    let takes = design(&state, &description, language.as_deref(), &DESIGN_CANCEL, &say).await?;
    let out = takes
        .iter()
        .map(|t| MadeSample { wav: base64::engine::general_purpose::STANDARD.encode(crate::audio::wav_encode(t, TTS_RATE)), seconds: t.len() as f32 / TTS_RATE as f32 })
        .collect();
    *MADE.lock().unwrap() = Some(Made { description, language, takes });
    state.log("media", "Made voices from a description");
    Ok(out)
}

/// Three readings of the sample in voices made from `description` (24 kHz).
pub async fn design(state: &Arc<AppState>, description: &str, language: Option<&str>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<Vec<f32>>, String> {
    let (exe, files) = design_parts(state, cancel, progress).await?;
    progress("Making the voices", 0.0);
    let words = sample_words(language).to_string();
    let lines = vec![words.clone(), words.clone(), words];
    let mut takes = run_engine(&exe, &files, How::Design(description), &lines, language, None, cancel).await?;
    if !heard_any(&takes) && exe.to_string_lossy().contains("vulkan") {
        crate::applog::line("media", "The graphics card's voice maker made only silence; making it again on the processor");
        takes = run_engine(&exe, &files, How::Design(description), &lines, language, Some("CPU"), cancel).await?;
    }
    takes.retain(|t| !t.is_empty() && t.iter().all(|s| s.is_finite()) && crate::audio::rms(t) > 0.003);
    if takes.is_empty() {
        return Err("The voice maker made only silence. Try again, or describe the voice another way.".into());
    }
    for t in &mut takes {
        t.truncate((longest_s(&lines[0]) * TTS_RATE as f32) as usize);
    }
    Ok(takes)
}

/// Keeps a made reading as a voice (for tests: the command keeps one of the
/// last made).
#[cfg(test)]
pub fn keep_for_test(state: &AppState, name: &str, description: &str, sample: &[f32]) -> String {
    *MADE.lock().unwrap() = Some(Made { description: description.into(), language: Some("en".into()), takes: vec![sample.to_vec()] });
    let at48 = stems::resample(sample, TTS_RATE, REC_RATE);
    let id = add_for_test(state, name, &[(&format!("{ADDED}1"), at48, sample_words(Some("en")))]);
    let c = state.work_cipher().unwrap();
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    if let Some(v) = all.iter_mut().find(|v| v.id == id) {
        v.designed = Some(description.into());
        for t in &mut v.takes {
            t.kind = Some(LineKind::Speak);
            t.score = 1.0;
        }
    }
    store(&conn, &c, &all).unwrap();
    id
}

#[tauri::command]
pub fn own_voice_design_cancel() {
    DESIGN_CANCEL.store(true, Ordering::Relaxed);
}

/// Keeps one of the voices just made (`pick`, from 0) under `name`: its
/// reading becomes the voice's reference, so it sounds the same every time.
#[tauri::command]
pub fn own_voice_design_keep(state: AppStateRef<'_>, name: String, pick: usize) -> Result<OwnVoicesView, String> {
    let name = name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err("Give the voice a name.".into());
    }
    let made = MADE.lock().unwrap().take().ok_or("Make the voices again first.")?;
    if pick >= made.takes.len() {
        *MADE.lock().unwrap() = Some(made);
        return Err("That voice isn't one of the ones made.".into());
    }
    let at48 = stems::resample(&made.takes[pick], TTS_RATE, REC_RATE);
    let c = state.work_cipher()?;
    let id = super::store::new_id();
    let line = format!("{ADDED}1");
    let path = take_path(&state.paths, &id, &line)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&path, c.seal_bytes(&crate::audio::wav_encode(&at48, REC_RATE))).map_err(|e| e.to_string())?;
    let now = db::now_ms();
    let take = Take {
        line,
        seconds: at48.len() as f32 / REC_RATE as f32,
        words: sample_words(made.language.as_deref()).to_string(),
        at: now,
        kind: Some(LineKind::Speak),
        score: 1.0,
        ..Default::default()
    };
    {
        let conn = state.db.lock().unwrap();
        let mut all = list(&conn, &c);
        all.push(OwnVoice { id, name, takes: vec![take], created_at: now, agreed_at: now, designed: Some(made.description) });
        store(&conn, &c, &all)?;
    }
    state.log("media", "Kept a voice made from a description");
    view(&state)
}

/// Hears a sentence in a voice, to check it: WAV, base64. "own_voice:trying"
/// events say what it's doing: downloading the engine or model the first
/// time (with how far), then reading.
#[tauri::command]
pub async fn own_voice_try(app: AppHandle, state: AppStateRef<'_>, voice: String, text: String, language: Option<String>) -> Result<String, String> {
    use base64::Engine as _;
    crate::features::require(&state, crate::features::Feature::Music)?;
    let state = state.inner().clone();
    let never = AtomicBool::new(false);
    let say = |stage: &str, fraction: f64| {
        // While it reads there's no telling how far it is (-1).
        let downloading = stage.contains("(once");
        let _ = app.emit("own_voice:trying", json!({ "stage": stage, "fraction": if downloading { fraction } else { -1.0 }, "downloading": downloading }));
    };
    let (samples, rate) = speak(&state, &voice, &text, language.as_deref(), &never, &say).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(crate::audio::wav_encode(&samples, rate)))
}

// ---------- singing in the voice ----------

/// Seed-VC's pieces, downloaded the first time.
pub async fn sing_parts(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<PathBuf, String> {
    let spec = state.catalog.media.own_voice.as_ref().map(|c| c.sing.clone()).ok_or("Singing in your voice isn't available on this system yet.")?;
    // Each piece kept under the name the converter knows ("dit.onnx").
    let files: Vec<MediaFile> = spec.files.iter().map(|f| MediaFile { file: format!("{}.onnx", f.role), ..f.clone() }).collect();
    let got = fetch_all(state, &files, "own-voice-sing", "Getting the singing voice model (once", cancel, progress).await?;
    Ok(got[0].parent().unwrap().to_path_buf())
}

/// 16-bit stereo WAV bytes.
pub fn wav_stereo(l: &[f32], r: &[f32], rate: u32) -> Vec<u8> {
    let n = l.len().min(r.len());
    let data_len = (n * 4) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        out.extend_from_slice(&crate::audio::to_i16(l[i]).to_le_bytes());
        out.extend_from_slice(&crate::audio::to_i16(r[i]).to_le_bytes());
    }
    out
}

/// The new voice at the old one's loudness, where each is sung, and the
/// two put over the band; the whole kept under clipping.
pub fn mix(band: &stems::Stereo, old_voice: &[f32], new_voice: &[f32]) -> stems::Stereo {
    let loud = |x: &[f32]| {
        let mut r: Vec<f32> = x.chunks(4410).map(crate::audio::rms).filter(|v| *v > 1e-3).collect();
        r.sort_by(|a, b| a.total_cmp(b));
        r.get(r.len() * 3 / 4).copied().unwrap_or(0.0)
    };
    let gain = (loud(old_voice) / loud(new_voice).max(1e-4)).clamp(0.1, 8.0);
    let n = band.len().min(new_voice.len());
    let mut ch = [band.ch[0][..n].to_vec(), band.ch[1][..n].to_vec()];
    for c in ch.iter_mut() {
        for (i, s) in c.iter_mut().enumerate() {
            *s += new_voice[i] * gain;
        }
    }
    let peak = ch.iter().flat_map(|c| c.iter()).fold(0f32, |m, s| m.max(s.abs()));
    if peak > 0.98 {
        let k = 0.98 / peak;
        ch.iter_mut().for_each(|c| c.iter_mut().for_each(|s| *s *= k));
    }
    stems::Stereo { rate: band.rate, ch }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_voice_is_a_mans_or_a_womans_by_its_pitch() {
        // A voice with overtones, gliding a little, between pauses.
        let voice = |hz: f32| -> Vec<f32> {
            (0..16_000 * 4)
                .map(|i| {
                    let t = i as f32 / 16_000.0;
                    let f = hz * (1.0 + 0.03 * (t * 2.0).sin());
                    let on = if (t * 2.0).fract() < 0.8 { 1.0 } else { 0.0 };
                    on * (1..5).map(|k| (std::f32::consts::TAU * f * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.2
                })
                .collect()
        };
        let speaking = |hz: f32| sex_by_pitch(middle_pitch(&voice(hz), 16_000).unwrap(), 160.0, 175.0);
        assert_eq!(speaking(120.0), Some("male"));
        assert_eq!(speaking(139.0), Some("male"));
        assert_eq!(speaking(215.0), Some("female"));
        assert_eq!(speaking(168.0), None, "in between: left to the description");
        assert_eq!(middle_pitch(&vec![0.0; 16_000], 16_000), None);
    }

    #[test]
    fn text_goes_in_sentence_lines() {
        let l = lines_of("Hello there. How are you?\n\nFine!");
        assert_eq!(l, vec!["Hello there. How are you?", "Fine!"]);
        let long = "This sentence is about forty characters. ".repeat(12);
        let l = lines_of(&long);
        assert!(l.len() >= 2 && l.iter().all(|x| x.len() <= 330), "{l:?}");
    }

    #[test]
    fn streamed_lines_are_split_at_their_headers() {
        let head = |n: usize| {
            let mut h = b"RIFF\xff\xff\xff\x7fWAVEfmt ".to_vec();
            h.resize(44, 0);
            h.extend(std::iter::repeat_n([0x00u8, 0x40], n).flatten());
            h
        };
        let mut b = head(3);
        b.extend(head(2));
        let parts = split_streamed(&b);
        assert_eq!(parts.iter().map(Vec::len).collect::<Vec<_>>(), vec![3, 2]);
        assert!((parts[0][0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn silent_or_broken_readings_are_caught() {
        let voice: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        assert!(heard_any(&[vec![0.0; 24_000], voice.clone()]));
        assert!(!heard_any(&[vec![0.0; 24_000], vec![]]));
        assert!(!heard_any(&[vec![0.0001; 24_000]]));
        let mut broken = voice;
        broken[100] = f32::NAN;
        assert!(!heard_any(&[broken]));
    }

    #[test]
    fn takes_are_trimmed_checked_and_levelled() {
        let rate = REC_RATE as usize;
        let mut s = vec![0.0005f32; rate];
        s.extend((0..rate * 3).map(|i| 0.3 * (i as f32 * 0.05).sin()));
        s.extend(vec![0.0005f32; rate]);
        let (take, notes) = clean(&s, 2.0).unwrap();
        let secs = take.len() as f32 / REC_RATE as f32;
        assert!((3.0..3.7).contains(&secs), "{secs}");
        assert!(notes.is_empty(), "{notes:?}");
        assert!(clean(&s, 5.0).unwrap_err().contains("only"));
        assert!(clean(&vec![0.0; rate * 2], 1.0).is_err());
        // All voice from the first moment: kept whole, and not called noisy.
        let loud: Vec<f32> = (0..rate * 3).map(|i| if i % 50 < 5 { 1.0 } else { 0.4 * (i as f32 * 0.05).sin() }).collect();
        let (all, notes) = clean(&loud, 1.0).unwrap();
        assert_eq!(all.len(), loud.len());
        assert_eq!(notes, vec!["loud".to_string()]);
        // A hum under every pause is noise.
        let mut hum: Vec<f32> = (0..rate).map(|i| 0.02 * (i as f32 * 0.3).sin()).collect();
        hum.extend((0..rate * 2).map(|i| 0.1 * (i as f32 * 0.05).sin() + 0.02 * (i as f32 * 0.3).sin()));
        hum.extend((0..rate).map(|i| 0.02 * (i as f32 * 0.3).sin()));
        assert!(clean(&hum, 1.0).unwrap().1.contains(&"noisy".to_string()));
    }

    #[test]
    fn voices_are_found_by_what_a_chat_calls_them() {
        let v = |id: &str, name: &str| OwnVoice { id: id.into(), name: name.into(), ..Default::default() };
        let voices = vec![v("a", "My voice"), v("b", "Grandpa Joe")];
        assert_eq!(find(&voices, "grandpa joe").map(|x| x.id.as_str()), Some("b"));
        assert_eq!(find(&voices, "Grandpa").map(|x| x.id.as_str()), Some("b"));
        assert_eq!(find(&voices, "me").map(|x| x.id.as_str()), Some("a"));
        assert!(find(&voices, "Mozart").is_none());
        assert!(find(&[], "my voice").is_none());
    }

    #[test]
    fn voices_know_what_they_can_do() {
        let t = |l: &str| Take { line: l.into(), seconds: 3.0, ..Default::default() };
        let mut v = OwnVoice { takes: vec![t("intro"), t("story")], ..Default::default() };
        assert!(!v.can_speak() && !v.can_sing());
        v.takes.push(t("calm"));
        assert!(v.can_speak() && v.can_sing());
        let only_singing = OwnVoice { takes: vec![t("song")], ..Default::default() };
        assert!(!only_singing.can_speak() && only_singing.can_sing());
    }

    /// A song sung again in another voice, the way `revoice_item` does it,
    /// written out for listening: SULCUSAI_SONG (default a Compose test
    /// song), the voice from SULCUSAI_VOICE (default qwentts.cpp's example
    /// speech), Seed-VC's pieces in SULCUSAI_SVC_DIR, the installed app's
    /// separator and ONNX Runtime (the card's with SULCUSAI_ONNX_CARD).
    /// Prints how much nearer the voice print came to the voice (CAMPPlus,
    /// the converter's own, so listen too).
    ///     cargo test sings_a_song_in_a_recorded_voice -- --ignored --nocapture
    #[test]
    #[ignore]
    fn sings_a_song_in_a_recorded_voice() {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        let song_path = PathBuf::from(env("SULCUSAI_SONG", r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3"));
        let voice_path = PathBuf::from(env("SULCUSAI_VOICE", r"C:\dev\tools\voice\qwentts.cpp\examples\freeman.wav"));
        let svc_dir = PathBuf::from(env("SULCUSAI_SVC_DIR", r"C:\dev\tools\voice\onnx16"));
        let out = PathBuf::from(env("SULCUSAI_OUT", r"C:\dev\tools\voice\tests\sung"));
        let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("app.sulcusai.desktop");
        let models = crate::storage::models_dir(&local);
        let card = std::env::var_os("SULCUSAI_ONNX_CARD").is_some();
        let rt = local.join("engines").join(if card { "onnxruntime-1.30.0-cuda13" } else { "onnxruntime-1.30.0-cpu-x64" });
        let dll = [rt.join("onnxruntime.dll"), rt.join("onnxruntime-win-x64-1.30.0").join("lib").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
        let card = card.then_some(crate::natural::Card { device: 0, dml: false });
        let decode = |p: &Path| {
            let (s, rate, ch) = super::super::audio_in::decode(p).unwrap();
            let ch = ch.max(1) as usize;
            stems::Stereo { rate, ch: [s.chunks(ch).map(|f| f[0]).collect(), s.chunks(ch).map(|f| f[if ch > 1 { 1 } else { 0 }]).collect()] }.resampled(stems::RATE)
        };
        let started = std::time::Instant::now();
        let song = decode(&song_path);
        let reference = decode(&voice_path).mono();
        let (parts, _) = stems::separate(&dll, &models.join("media").join("helpers").join("htdemucs_fp16weights.onnx"), &song, 8, &[0, 1, 2, 3], card, &AtomicBool::new(false)).unwrap();
        stems::unload();
        let apart = started.elapsed().as_secs_f64();
        let band = stems::Stereo { rate: stems::RATE, ch: [0, 1].map(|c| (0..song.len()).map(|i| parts[0].ch[c][i] + parts[1].ch[c][i] + parts[2].ch[c][i]).collect()) };
        let sung = parts[3].mono();
        let mut conv = svc::Converter::new(&dll, &svc_dir, 8, card).unwrap();
        let v = conv.voice(&reference).unwrap();
        let song_pitch = conv.middle_pitch_of(&sung).unwrap();
        let semitones = svc::octave_shift(song_pitch, v.pitch * 1.26);
        let steps = if card.is_some() { 30 } else { 10 };
        let t = std::time::Instant::now();
        let never = AtomicBool::new(false);
        let new_voice = conv.convert(&v, &sung, svc::Settings { steps, semitones, ..Default::default() }, &never, &|_| {}).unwrap();
        println!(
            "{:.0} s song: apart in {apart:.0} s, sung again in {:.0} s ({} steps, {semitones} semitones: song {song_pitch:.0} Hz, voice {:.0} Hz) on the {}",
            song.len() as f64 / stems::RATE as f64,
            t.elapsed().as_secs_f64(),
            steps,
            v.pitch,
            if conv.on_card { "card" } else { "processor" }
        );
        let mixed = mix(&band, &sung, &new_voice);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("as-made.wav"), wav_stereo(&song.ch[0], &song.ch[1], stems::RATE)).unwrap();
        std::fs::write(out.join("in-the-voice.wav"), wav_stereo(&mixed.ch[0], &mixed.ch[1], stems::RATE)).unwrap();
        std::fs::write(out.join("voice-alone.wav"), crate::audio::wav_encode(&new_voice, stems::RATE)).unwrap();
        // Voice prints of the voice, the song's singer and the new singer.
        let mut print = |x: &[f32]| {
            let w16 = stems::resample(x, stems::RATE, 16_000);
            let (n, fb) = crate::diarize::fbank_with(&w16, true);
            let e = conv.run("campplus.onnx", vec![("fbank", vec![1, n as i64, 80], fb)]).unwrap();
            let norm = e.iter().map(|v| v * v).sum::<f32>().sqrt();
            e.into_iter().map(|v| v / norm).collect::<Vec<f32>>()
        };
        let (target, before, after) = (print(&reference), print(&sung), print(&new_voice));
        let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        println!("voice print likeness to the voice: song's singer {:.2}, sung again {:.2}", cos(&before, &target), cos(&after, &target));
        assert!(cos(&after, &target) > cos(&before, &target));
    }

    /// Narration in a recorded voice through the engine, as `speak` runs it:
    /// qwentts.cpp in SULCUSAI_TTS_DIR (default the graphics-card build),
    /// its models in SULCUSAI_TTS_MODELS, the voice from qwentts.cpp's
    /// example speech and its words. Prints the time and how near the voice
    /// print came (CAMPPlus, from Seed-VC's pieces in SULCUSAI_SVC_DIR);
    /// writes the reading out for listening.
    ///     cargo test reads_in_a_recorded_voice -- --ignored --nocapture
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn reads_in_a_recorded_voice() {
        let env = |k: &str, d: &str| PathBuf::from(std::env::var(k).unwrap_or_else(|_| d.to_string()));
        let dir = env("SULCUSAI_TTS_DIR", r"C:\dev\qb");
        let models = env("SULCUSAI_TTS_MODELS", r"C:\dev\tools\voice\models");
        let example = PathBuf::from(r"C:\dev\tools\voice\qwentts.cpp\examples");
        let big = std::env::var_os("SULCUSAI_TTS_SMALL").is_none();
        let files = [models.join(if big { "qwen-talker-1.7b-base-Q8_0.gguf" } else { "qwen-talker-0.6b-base-Q8_0.gguf" }), models.join("qwen-tokenizer-12hz-Q8_0.gguf")];
        let pcm = crate::audio::wav_decode(&std::fs::read(example.join("freeman.wav")).unwrap()).unwrap();
        let reference = stems::resample(&pcm.samples, pcm.rate, TTS_RATE);
        let words = std::fs::read_to_string(example.join("freeman.txt")).unwrap();
        let lines = lines_of("Your package from the bakery arrives on Thursday afternoon. Please leave the side gate open, and the dog inside!");
        let t = std::time::Instant::now();
        let never = AtomicBool::new(false);
        let (said, rate) = read_with(&dir.join(TTS_EXE), &files, &reference, words.trim(), &lines, Some("en"), None, &never).await.unwrap();
        let secs = said.len() as f64 / rate as f64;
        println!("{} lines, {secs:.1} s of speech in {:.1} s ({})", lines.len(), t.elapsed().as_secs_f64(), if big { "1.7B" } else { "0.6B" });
        assert!(secs > 3.0);
        let out = env("SULCUSAI_OUT", r"C:\dev\tools\voice\tests");
        std::fs::write(out.join("read-in-the-voice.wav"), crate::audio::wav_encode(&said, rate)).unwrap();
        // Voice prints: the reading against the voice, and against a voice
        // of Windows' own reading the same words.
        let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("app.sulcusai.desktop");
        let rt = local.join("engines").join("onnxruntime-1.30.0-cpu-x64");
        let dll = [rt.join("onnxruntime.dll"), rt.join("onnxruntime-win-x64-1.30.0").join("lib").join("onnxruntime.dll")].into_iter().find(|p| p.is_file()).unwrap();
        let mut conv = svc::Converter::new(&dll, &env("SULCUSAI_SVC_DIR", r"C:\dev\tools\voice\onnx16"), 8, None).unwrap();
        let mut print = |x: &[f32], from: u32| {
            let w16 = stems::resample(x, from, 16_000);
            let (n, fb) = crate::diarize::fbank_with(&w16, true);
            let e = conv.run("campplus.onnx", vec![("fbank", vec![1, n as i64, 80], fb)]).unwrap();
            let norm = e.iter().map(|v| v * v).sum::<f32>().sqrt();
            e.into_iter().map(|v| v / norm).collect::<Vec<f32>>()
        };
        let other = crate::tts::synthesize(&lines.join(" "), None, Some("en"), 1.0).unwrap();
        let other_samples: Vec<f32> = other.samples.iter().map(|s| *s as f32 / 32768.0).collect();
        let (target, clone, windows) = (print(&reference, TTS_RATE), print(&said, rate), print(&other_samples, other.rate));
        let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        println!("voice print likeness to the voice: the reading {:.2}, Windows' voice {:.2}", cos(&clone, &target), cos(&windows, &target));
        assert!(cos(&clone, &target) > cos(&windows, &target));
    }

    /// The installed app's recorded voices read aloud the way Your voice's
    /// "Read it in this voice" does, from its own data (read only: the
    /// database opened read-only, takes decrypted in memory), its engine and
    /// models. Prints each voice's takes (lines, lengths, notes; not the
    /// words), the reference and the reading or the error; writes the
    /// reading to SULCUSAI_OUT for listening.
    ///     cargo test real_own_voice_reads -- --ignored --nocapture
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn real_own_voice_reads() {
        let data = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("app.sulcusai.desktop");
        let models = crate::storage::models_dir(&data);
        let paths = Paths { models: models.clone(), engines: data.join("engines"), logs: data.join("logs"), db: data.join("sulcusai.db"), data: data.clone() };
        let conn = Connection::open_with_flags(&paths.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let vault = crate::crypto::Vault::open(&data.join("keys.json"), crate::crypto::platform_protector(&data)).unwrap();
        let c = vault.work_cipher().expect("the app's data key is locked with a password");
        let voices = list(&conn, &c);
        println!("{} voice(s)", voices.len());
        for v in &voices {
            println!("voice \"{}\": can speak {}, can sing {}", v.name, v.can_speak(), v.can_sing());
            for t in &v.takes {
                println!("  {}: {:.1} s, {} words, notes {:?}", t.line, t.seconds, t.words.split_whitespace().count(), t.notes);
            }
            let (reference, words) = match speech_reference(&paths, &c, v) {
                Ok(r) => r,
                Err(e) => {
                    println!("  reference: {e}");
                    continue;
                }
            };
            println!("  reference: {:.1} s, {} words", reference.len() as f32 / TTS_RATE as f32, words.split_whitespace().count());
            let exe = paths.engines.join("qwentts-5916bf7-vulkan-x64").join(TTS_EXE);
            let files = [models.join("media").join("own-voice").join("qwen-talker-1.7b-base-Q8_0.gguf"), models.join("media").join("own-voice").join("qwen-tokenizer-12hz-Q8_0.gguf")];
            let lines = lines_of("This is how I sound when SulcusAI reads something for me.");
            let never = AtomicBool::new(false);
            let t = std::time::Instant::now();
            // SULCUSAI_TTS_DEVICE=CPU: on the processor, as when the card is full.
            let device = std::env::var("SULCUSAI_TTS_DEVICE").ok();
            match read_with(&exe, &files, &reference, &words, &lines, Some("en"), device.as_deref(), &never).await {
                Ok((said, rate)) => {
                    println!("  read: {:.1} s of speech in {:.1} s", said.len() as f32 / rate as f32, t.elapsed().as_secs_f64());
                    let out = PathBuf::from(std::env::var("SULCUSAI_OUT").unwrap_or_else(|_| r"C:\dev\tools\voice\tests".into()));
                    std::fs::write(out.join(format!("real-{}.wav", v.id)), crate::audio::wav_encode(&said, rate)).unwrap();
                }
                Err(e) => println!("  read failed: {e}"),
            }
        }
    }

    /// Recordings added to a voice, as Your voice's "Add recordings" does: a
    /// song (its voice taken out of the music) as singing and the example
    /// speech as speaking, in a throwaway app data folder (the separator
    /// and its runtime from the real ones). SULCUSAI_SONG picks the song.
    ///     cargo test songs_and_speech_are_added -- --ignored --nocapture
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn songs_and_speech_are_added_to_a_voice() {
        let state = crate::e2e::temp_state(None);
        let c = state.work_cipher().unwrap();
        let id = add_for_test(&state, "Added", &[]);
        let quiet = |_: &str| {};
        let song = PathBuf::from(std::env::var("SULCUSAI_SONG").unwrap_or_else(|_| r"C:\dev\tools\compose-merge\own-words-real-0.375.mp3".into()));
        let speech = PathBuf::from(r"C:\dev\tools\voice\qwentts.cpp\examples\freeman.wav");
        let t = std::time::Instant::now();
        add_one(&state, &c, &id, &song, LineKind::Sing, &quiet).await.unwrap();
        println!("song added in {:.0} s", t.elapsed().as_secs_f64());
        add_one(&state, &c, &id, &speech, LineKind::Speak, &quiet).await.unwrap();
        // Too short to use.
        let short = std::env::temp_dir().join(format!("sulcusai-short-{}.wav", super::super::store::new_id()));
        std::fs::write(&short, crate::audio::wav_encode(&vec![0.1f32; 32_000], 16_000)).unwrap();
        assert!(add_one(&state, &c, &id, &short, LineKind::Sing, &quiet).await.unwrap_err().contains("shorter"));
        let _ = std::fs::remove_file(&short);
        let v = get(&state.db.lock().unwrap(), &c, &id).unwrap();
        for t in &v.takes {
            println!("{} ({:?}, {}): {:.1} s, voice {:.0}%, notes {:?}, {} words", t.line, t.kind, t.file, t.seconds, t.score * 100.0, t.notes, t.words.split_whitespace().count());
        }
        assert_eq!(v.takes.len(), 2);
        assert!(v.can_sing() && v.can_speak(), "an added song sings and 15 s of speech reads");
        let (sing_ref, sung) = singing_reference(&state.paths, &c, &v).unwrap();
        assert!(sung && sing_ref.len() as f32 / svc::RATE as f32 > 8.0);
        let (speak_ref, _) = speech_reference(&state.paths, &c, &v).unwrap();
        assert!(speak_ref.len() as f32 / TTS_RATE as f32 > 8.0);
    }

    #[test]
    fn the_clearest_stretch_is_where_the_voice_is() {
        let rate = 1000;
        // 20 s quiet, 10 s of voice, 20 s quiet: a 6 s stretch lands inside the voice.
        let mut s = vec![0.0005f32; rate * 20];
        s.extend((0..rate * 10).map(|i| 0.3 * (i as f32 * 0.7).sin()));
        s.extend(vec![0.0005f32; rate * 20]);
        let (a, b, score) = clearest(&s, rate as u32, 6.0);
        assert!(a >= rate * 20 && b <= rate * 30, "{a}..{b}");
        assert_eq!(b - a, rate * 6);
        assert!(score > 0.95, "{score}");
        // Shorter than asked: all of it.
        let (a, b, _) = clearest(&s[..rate * 3], rate as u32, 6.0);
        assert_eq!((a, b), (0, rate * 3));
    }

    #[test]
    fn voices_saved_before_added_recordings_read_the_same() {
        // A voice as 0.9.34 saved it: no kind, file or score on its takes.
        let old = r#"[{"id":"v1","name":"Ike","takes":[{"line":"intro","seconds":6.5,"words":"Hi","notes":[],"at":1},{"line":"story","seconds":6.9,"words":"The","notes":[],"at":2},{"line":"calm","seconds":6.3,"words":"Take","notes":[],"at":3},{"line":"song","seconds":24.1,"words":"","notes":[],"at":4}],"created_at":1,"agreed_at":1}]"#;
        let voices: Vec<OwnVoice> = serde_json::from_str(old).unwrap();
        let v = &voices[0];
        assert!(v.can_speak() && v.can_sing());
        assert_eq!(v.order(LineKind::Sing, SING_ORDER)[0], "song");
        assert_eq!(v.order(LineKind::Speak, SPEAK_ORDER)[0], "everyday");
        // Added recordings go first, the clearest before the rest; the script's takes stay.
        let mut v = v.clone();
        let added = |line: &str, score: f32, kind: LineKind, seconds: f32| Take { line: line.into(), kind: Some(kind), score, seconds, ..Default::default() };
        v.takes.push(added("added-1", 0.6, LineKind::Sing, 15.0));
        v.takes.push(added("added-2", 0.9, LineKind::Sing, 15.0));
        assert_eq!(&v.order(LineKind::Sing, SING_ORDER)[..3], ["added-2", "added-1", "song"]);
        assert_eq!(v.takes.len(), 6);
        // One added reading of 8 s is enough to narrate.
        let only = OwnVoice { takes: vec![added("added-1", 0.8, LineKind::Speak, 9.0)], ..Default::default() };
        assert!(only.can_speak());
    }

    #[test]
    fn a_line_may_take_about_its_length() {
        // 72 characters: 12 s at most, while a natural reading is ~5 s.
        let line = "The bakery opens at seven. Bring the blue umbrella, it might rain later.";
        assert!((longest_s(line) - 12.0).abs() < 0.1, "{}", longest_s(line));
        assert!((longest_s("Hi.") - 3.375).abs() < 0.01);
    }

    #[test]
    fn languages_get_the_engines_names() {
        assert_eq!(tts_language(Some("en-US")), "English");
        assert_eq!(tts_language(Some("pt")), "Portuguese");
        assert_eq!(tts_language(Some("nl")), "auto");
        assert_eq!(tts_language(None), "auto");
    }

    #[test]
    fn the_new_voice_sits_where_the_old_one_was() {
        let band = stems::Stereo { rate: 44_100, ch: [vec![0.1; 44_100], vec![0.1; 44_100]] };
        let old: Vec<f32> = (0..44_100).map(|i| 0.2 * (i as f32 * 0.03).sin()).collect();
        let new: Vec<f32> = old.iter().map(|x| x * 0.25).collect();
        let m = mix(&band, &old, &new);
        let voice: Vec<f32> = m.ch[0].iter().map(|x| x - 0.1).collect();
        let r = |x: &[f32]| crate::audio::rms(x);
        assert!((r(&voice) / r(&old) - 1.0).abs() < 0.05, "{}", r(&voice) / r(&old));
    }
}

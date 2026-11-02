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

mod narrate;
#[cfg(test)]
mod tests;
pub use narrate::*;

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
/// kHz; and whether it was singing. The song line comes first: a song sung
/// again from it sounded more like the person than from the soft-to-loud
/// and scale lines, though those reach more of their range.
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

/// All of a voice's singing (up to two minutes, at Seed-VC's rate), for
/// measuring the notes it reaches (`svc::range_of`; the middle pitch above
/// takes overtones for notes too often to give a range). None without
/// singing.
pub fn all_singing(paths: &Paths, c: &Cipher, voice: &OwnVoice) -> Option<Vec<f32>> {
    voice.takes_of(LineKind::Sing).next()?;
    joined(paths, c, voice, &voice.order(LineKind::Sing, SING_ORDER), svc::RATE, 120.0).ok().map(|(s, _)| s)
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

/// A recorded voice copied, takes and all, from another copy of the app's
/// data (the installed app's, read only) into `state`: for tests on a real
/// person's voice that change nothing of theirs.
#[cfg(test)]
pub fn copy_for_test(from: &Paths, from_cipher: &Cipher, voice: &OwnVoice, state: &AppState) -> String {
    let c = state.work_cipher().unwrap();
    let id = super::store::new_id();
    let mut copy = OwnVoice { id: id.clone(), takes: Vec::new(), ..voice.clone() };
    for t in &voice.takes {
        let Ok(samples) = read_take(from, from_cipher, &voice.id, &t.line) else { continue };
        let path = take_path(&state.paths, &id, &t.line).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, c.seal_bytes(&crate::audio::wav_encode(&samples, REC_RATE))).unwrap();
        copy.takes.push(t.clone());
    }
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn, &c);
    all.push(copy);
    store(&conn, &c, &all).unwrap();
    id
}

/// Each take of a voice with its line and kind (48 kHz): for tests.
#[cfg(test)]
pub fn takes_for_test(paths: &Paths, c: &Cipher, voice: &OwnVoice) -> Vec<(String, Option<LineKind>, Vec<f32>)> {
    voice.takes.iter().filter_map(|t| Some((t.line.clone(), t.kind(), read_take(paths, c, &voice.id, &t.line).ok()?))).collect()
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

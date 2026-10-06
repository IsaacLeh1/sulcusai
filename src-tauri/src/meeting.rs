// SPDX-License-Identifier: AGPL-3.0-only
//! Meeting mode: records the microphone ("You") and the computer's audio
//! ("Others") on separate channels, transcribes as the meeting goes, and
//! writes notes at the end. Works with any call app or in person, with no
//! bot joining the call. Transcripts, notes and audio are stored encrypted.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::audio::{self, Capture, Source};
use crate::chat;
use crate::crypto::Cipher;
use crate::db;
use crate::engine::Endpoint;
use crate::speech::{self, SpeechEndpoint};
use crate::vad::{self, Phrase, Phraser};
use crate::{AppState, AppStateRef};

/// Audio is stored in encrypted chunks of this many seconds per channel.
const CHUNK_SECS: usize = 30;
const CHUNK: usize = CHUNK_SECS * audio::RATE as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    /// The microphone: the user (and anyone in the room with them).
    You,
    /// The computer's audio: the other people in a call.
    Others,
}

impl Speaker {
    fn as_str(self) -> &'static str {
        match self {
            Speaker::You => "you",
            Speaker::Others => "others",
        }
    }

    fn parse(s: &str) -> Speaker {
        if s == "you" { Speaker::You } else { Speaker::Others }
    }

    fn label(self) -> &'static str {
        match self {
            Speaker::You => "You",
            Speaker::Others => "Others",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActionItem {
    pub task: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub due: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Notes {
    pub title: String,
    pub summary: String,
    pub topics: Vec<String>,
    pub key_points: Vec<String>,
    pub most_important: Vec<String>,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItem>,
}

/// The encrypted part of a meeting record.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MeetingData {
    pub title: String,
    /// The user named it; notes don't replace the title.
    pub titled_by_user: bool,
    pub notes: Option<Notes>,
    pub translate_to: Option<String>,
    pub has_audio: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Meeting {
    pub id: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// "recording", "transcribing", "summarizing", "done" or "failed".
    pub status: String,
    #[serde(flatten)]
    pub data: MeetingData,
}

#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub id: i64,
    pub speaker: Speaker,
    /// Seconds from the start of the meeting.
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub translation: Option<String>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---------- storage ----------

fn load(conn: &Connection, c: &Cipher, id: &str) -> Option<Meeting> {
    conn.query_row("SELECT id, data, started_at, ended_at, status FROM meetings WHERE id = ?1", [id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .optional()
    .ok()
    .flatten()
    .map(|(id, data, started_at, ended_at, status)| Meeting {
        id,
        data: c.decrypt(&data).ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
        started_at,
        ended_at,
        status,
    })
}

pub fn list(conn: &Connection, c: &Cipher) -> Vec<Meeting> {
    let Ok(mut stmt) = conn.prepare("SELECT id FROM meetings ORDER BY started_at DESC") else { return Vec::new() };
    let ids: Vec<String> = stmt.query_map([], |r| r.get(0)).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_default();
    ids.iter().filter_map(|id| load(conn, c, id)).collect()
}

fn save_data(conn: &Connection, c: &Cipher, id: &str, data: &MeetingData) -> Result<(), String> {
    let json = serde_json::to_string(data).map_err(err)?;
    conn.execute("UPDATE meetings SET data = ?2 WHERE id = ?1", params![id, c.encrypt(&json)]).map_err(err)?;
    Ok(())
}

fn set_status(conn: &Connection, id: &str, status: &str) -> Result<(), String> {
    conn.execute("UPDATE meetings SET status = ?2 WHERE id = ?1", params![id, status]).map_err(err)?;
    Ok(())
}

pub(crate) fn create(conn: &Connection, c: &Cipher, data: &MeetingData) -> Result<Meeting, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = db::now_ms();
    let json = serde_json::to_string(data).map_err(err)?;
    conn.execute(
        "INSERT INTO meetings (id, data, started_at, status) VALUES (?1, ?2, ?3, 'recording')",
        params![id, c.encrypt(&json), now],
    )
    .map_err(err)?;
    Ok(Meeting { id, started_at: now, ended_at: None, status: "recording".into(), data: data.clone() })
}

pub fn segments(conn: &Connection, c: &Cipher, meeting_id: &str) -> Vec<Segment> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, speaker, start, end, text, translation FROM meeting_segments WHERE meeting_id = ?1 ORDER BY start, id",
    ) else {
        return Vec::new();
    };
    stmt.query_map([meeting_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?))
    })
    .map(|rows| {
        rows.filter_map(Result::ok)
            .map(|(id, sp, start, end, text, tr)| Segment {
                id,
                speaker: Speaker::parse(&sp),
                start,
                end,
                text: c.decrypt_or(&text, ""),
                translation: tr.map(|t| c.decrypt_or(&t, "")),
            })
            .collect()
    })
    .unwrap_or_default()
}

fn add_segment(conn: &Connection, c: &Cipher, meeting_id: &str, sp: Speaker, start: f64, end: f64, text: &str) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO meeting_segments (meeting_id, speaker, start, end, text) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![meeting_id, sp.as_str(), start, end, c.encrypt(text)],
    )
    .map_err(err)?;
    Ok(conn.last_insert_rowid())
}

pub fn audio_dir(state: &AppState, id: &str) -> PathBuf {
    state.paths.data.join("meetings").join(id)
}

// ---------- audio files ----------

/// Writes one channel to encrypted chunk files as it is recorded.
struct AudioWriter {
    dir: PathBuf,
    speaker: Speaker,
    cipher: Cipher,
    buf: Vec<i16>,
    index: u32,
}

impl AudioWriter {
    fn new(dir: &Path, speaker: Speaker, cipher: Cipher) -> AudioWriter {
        AudioWriter { dir: dir.to_path_buf(), speaker, cipher, buf: Vec::with_capacity(CHUNK), index: 0 }
    }

    fn push(&mut self, samples: &[f32]) {
        self.buf.extend(samples.iter().map(|s| audio::to_i16(*s)));
        while self.buf.len() >= CHUNK {
            let rest = self.buf.split_off(CHUNK);
            self.write();
            self.buf = rest;
        }
    }

    fn write(&mut self) {
        let bytes: Vec<u8> = self.buf.iter().flat_map(|s| s.to_le_bytes()).collect();
        let path = self.dir.join(format!("{}-{:05}.enc", self.speaker.as_str(), self.index));
        if let Err(e) = std::fs::write(&path, self.cipher.seal_bytes(&bytes)) {
            eprintln!("saving meeting audio failed: {e}");
        }
        self.index += 1;
        self.buf.clear();
    }

    fn finish(mut self) {
        if !self.buf.is_empty() {
            self.write();
        }
    }
}

/// One channel's audio between two times, as floats.
fn read_channel(dir: &Path, sp: Speaker, c: &Cipher, start: f64, end: f64) -> Vec<f32> {
    let first = (start.max(0.0) * audio::RATE as f64) as usize;
    let last = (end.max(start) * audio::RATE as f64) as usize;
    let mut out = Vec::with_capacity(last - first);
    let mut chunk_index = first / CHUNK;
    let mut at = first;
    while at < last {
        let path = dir.join(format!("{}-{:05}.enc", sp.as_str(), chunk_index));
        let Ok(blob) = std::fs::read(&path) else { break };
        let Ok(bytes) = c.open_bytes(&blob) else { break };
        let samples: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let base = chunk_index * CHUNK;
        let from = at - base;
        let to = (last - base).min(samples.len());
        if from >= to {
            break;
        }
        out.extend(samples[from..to].iter().map(|s| *s as f32 / 32768.0));
        at = base + to;
        chunk_index += 1;
    }
    out
}

/// Both channels mixed, as WAV bytes.
pub fn clip(dir: &Path, c: &Cipher, start: f64, end: f64) -> Vec<u8> {
    let a = read_channel(dir, Speaker::You, c, start, end);
    let b = read_channel(dir, Speaker::Others, c, start, end);
    let n = a.len().max(b.len());
    let mixed: Vec<f32> = (0..n).map(|i| (a.get(i).copied().unwrap_or(0.0) + b.get(i).copied().unwrap_or(0.0)).clamp(-1.0, 1.0)).collect();
    audio::wav_encode(&mixed, audio::RATE)
}

// ---------- echo ----------

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// The microphone often picks up the call from the speakers. A "You" line
/// that says the same thing as an "Others" line at the same moment is that
/// echo, not the user.
pub fn is_echo(mine: &str, theirs: &str) -> bool {
    let a = words(mine);
    let b = words(theirs);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let set_b: std::collections::HashSet<&String> = b.iter().collect();
    let common = a.iter().filter(|w| set_b.contains(w)).count();
    common as f64 / a.len().min(b.len()).max(1) as f64 >= 0.6 && a.len().min(b.len()) >= 2
}

fn overlaps(a: (f64, f64), b: (f64, f64)) -> bool {
    a.0 < b.1 + 1.5 && b.0 < a.1 + 1.5
}

// ---------- recording ----------

pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Several utterances from one channel sent as one request: whisper pays
/// for a whole 30 s window per request, so short phrases are batched.
struct Job {
    speaker: Speaker,
    samples: Vec<f32>,
    /// (seconds into `samples`, seconds into the meeting, length) per utterance.
    pieces: Vec<(f64, f64, f64)>,
}

impl Job {
    /// Maps a time in the batch back to the meeting's clock.
    fn real_time(&self, t: f64) -> f64 {
        let piece = self.pieces.iter().rev().find(|p| t >= p.0).or(self.pieces.first());
        match piece {
            Some(&(at, real, len)) => real + (t - at).clamp(0.0, len),
            None => t,
        }
    }
}

/// Silence placed between batched utterances.
const GAP: usize = audio::RATE as usize / 2;
/// Send a batch once it holds this much speech…
const BATCH_SECS: f64 = 18.0;
/// …or once the channel has been quiet this long.
const BATCH_QUIET_SECS: f64 = 2.0;

#[derive(Default)]
struct Batch {
    samples: Vec<f32>,
    pieces: Vec<(f64, f64, f64)>,
    /// Meeting time (samples) where the last utterance ended.
    end: u64,
}

impl Batch {
    fn add(&mut self, u: vad::Utterance) {
        if !self.samples.is_empty() {
            self.samples.extend(std::iter::repeat(0.0).take(GAP));
        }
        let at = self.samples.len() as f64 / audio::RATE as f64;
        self.pieces.push((at, u.start_secs(), u.secs()));
        self.end = u.start + u.samples.len() as u64;
        self.samples.extend(u.samples);
    }

    fn secs(&self) -> f64 {
        self.samples.len() as f64 / audio::RATE as f64
    }

    fn take(&mut self, speaker: Speaker) -> Option<Job> {
        if self.samples.is_empty() {
            return None;
        }
        let b = std::mem::take(self);
        Some(Job { speaker, samples: b.samples, pieces: b.pieces })
    }
}

#[derive(Clone)]
struct Recent {
    id: i64,
    speaker: Speaker,
    start: f64,
    end: f64,
    text: String,
}

/// Transcribes utterances as they arrive and stores them, dropping echo.
async fn transcriber(
    state: Arc<AppState>,
    cipher: Cipher,
    meeting_id: String,
    ep: SpeechEndpoint,
    translate_to: Option<String>,
    emit: Emit,
    mut rx: mpsc::UnboundedReceiver<Job>,
) {
    let mut recent: VecDeque<Recent> = VecDeque::new();
    let mut context: std::collections::HashMap<Speaker, String> = Default::default();
    while let Some(job) = rx.recv().await {
        let prompt = context.get(&job.speaker).cloned();
        let t = match speech::transcribe(&ep, &job.samples, &speech::Options { prompt, ..Default::default() }).await {
            Ok(t) => t,
            Err(e) => {
                emit("warning", json!({ "message": e }));
                continue;
            }
        };
        // One line per segment whisper found, on the meeting's clock.
        let lines: Vec<(f64, f64, String)> = if t.segments.is_empty() {
            let total = job.samples.len() as f64 / audio::RATE as f64;
            vec![(job.real_time(0.0), job.real_time(total), t.text.clone())]
        } else {
            t.segments.iter().map(|s| (job.real_time(s.start), job.real_time(s.end), s.text.clone())).collect()
        };
        let lines = whole_sentences(lines);
        for (start, end, text) in lines {
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let end = end.max(start);
            if job.speaker == Speaker::You
                && recent.iter().any(|r| r.speaker == Speaker::Others && overlaps((start, end), (r.start, r.end)) && is_echo(&text, &r.text))
            {
                continue;
            }
            if job.speaker == Speaker::Others {
                let echoes: Vec<i64> = recent
                    .iter()
                    .filter(|r| r.speaker == Speaker::You && overlaps((start, end), (r.start, r.end)) && is_echo(&r.text, &text))
                    .map(|r| r.id)
                    .collect();
                for id in echoes {
                    let _ = state.db.lock().unwrap().execute("DELETE FROM meeting_segments WHERE id = ?1", [id]);
                    recent.retain(|r| r.id != id);
                    emit("removed", json!({ "segment_id": id }));
                }
            }
            let id = match add_segment(&state.db.lock().unwrap(), &cipher, &meeting_id, job.speaker, start, end, &text) {
                Ok(id) => id,
                Err(e) => {
                    emit("warning", json!({ "message": e }));
                    continue;
                }
            };
            let ctx = context.entry(job.speaker).or_default();
            ctx.push(' ');
            ctx.push_str(&text);
            if ctx.len() > 600 {
                *ctx = ctx[ctx.len() - 400..].to_string();
            }
            recent.push_back(Recent { id, speaker: job.speaker, start, end, text: text.clone() });
            while recent.len() > 40 {
                recent.pop_front();
            }
            emit(
                "segment",
                json!({ "segment": Segment { id, speaker: job.speaker, start, end, text: text.clone(), translation: None } }),
            );
            if let Some(lang) = translate_to.clone() {
                let (state, cipher, emit) = (state.clone(), cipher.clone(), emit.clone());
                tauri::async_runtime::spawn(async move {
                    if let Ok((ep, _)) = crate::background_endpoint(&state).await {
                        if let Ok(tr) = crate::translate::translate_text(&ep, &text, &lang).await {
                            let _ = state.db.lock().unwrap().execute(
                                "UPDATE meeting_segments SET translation = ?2 WHERE id = ?1",
                                params![id, cipher.encrypt(&tr)],
                            );
                            emit("translation", json!({ "segment_id": id, "text": tr }));
                        }
                    }
                });
            }
        }
    }
}

/// Whisper breaks lines wherever its timestamps fall, often mid-sentence;
/// rejoin pieces that don't end a sentence and follow straight on.
fn whole_sentences(lines: Vec<(f64, f64, String)>) -> Vec<(f64, f64, String)> {
    let mut out: Vec<(f64, f64, String)> = Vec::new();
    for (start, end, text) in lines {
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if let Some(prev) = out.last_mut() {
            let ended = prev.2.ends_with(['.', '!', '?', '…', '。', '？', '！']);
            if !ended && start - prev.1 < 1.0 {
                prev.2.push(' ');
                prev.2.push_str(&text);
                prev.1 = end;
                continue;
            }
        }
        out.push((start, end, text));
    }
    out
}

pub struct Input {
    pub speaker: Speaker,
    pub rx: mpsc::UnboundedReceiver<Vec<f32>>,
}

/// Records until `stop`, transcribing as it goes. Returns once everything
/// said has been written down.
#[allow(clippy::too_many_arguments)]
pub async fn record(
    state: &Arc<AppState>,
    cipher: &Cipher,
    meeting_id: &str,
    ep: SpeechEndpoint,
    mut inputs: Vec<Input>,
    keep_audio: bool,
    translate_to: Option<String>,
    stop: &AtomicBool,
    levels: &(dyn Fn() -> Value + Sync),
    emit: Emit,
) -> Result<(), String> {
    let (job_tx, job_rx) = mpsc::unbounded_channel::<Job>();
    let worker = tauri::async_runtime::spawn(transcriber(
        state.clone(),
        cipher.clone(),
        meeting_id.to_string(),
        ep,
        translate_to,
        emit.clone(),
        job_rx,
    ));
    let dir = audio_dir(state, meeting_id);
    if keep_audio {
        std::fs::create_dir_all(&dir).map_err(err)?;
    }
    // Meetings have shorter pauses than dictation; long turns are cut so the
    // transcript keeps up.
    let cfg = vad::Config { end_ms: 600, ..Default::default() };
    let mut phrasers: Vec<Phraser> = inputs.iter().map(|_| Phraser::new(cfg, 25.0)).collect();
    let mut writers: Vec<Option<AudioWriter>> =
        inputs.iter().map(|i| keep_audio.then(|| AudioWriter::new(&dir, i.speaker, cipher.clone()))).collect();
    let mut closed = vec![false; inputs.len()];
    let mut last_level = Instant::now();
    let mut batches: Vec<Batch> = inputs.iter().map(|_| Batch::default()).collect();

    let speakers: Vec<Speaker> = inputs.iter().map(|i| i.speaker).collect();
    let take = |i: usize, chunk: &[f32], phrasers: &mut Vec<Phraser>, writers: &mut Vec<Option<AudioWriter>>, batches: &mut Vec<Batch>| {
        if let Some(w) = &mut writers[i] {
            w.push(chunk);
        }
        for p in phrasers[i].push(chunk) {
            if let Phrase::Done(u) = p {
                batches[i].add(u);
                if batches[i].secs() >= BATCH_SECS {
                    if let Some(job) = batches[i].take(speakers[i]) {
                        let _ = job_tx.send(job);
                    }
                }
            }
        }
        // A pause in the conversation: send what this channel has.
        let quiet = (phrasers[i].position().saturating_sub(batches[i].end)) as f64 / audio::RATE as f64;
        if !phrasers[i].speaking() && quiet >= BATCH_QUIET_SECS {
            if let Some(job) = batches[i].take(speakers[i]) {
                let _ = job_tx.send(job);
            }
        }
    };

    while !stop.load(Ordering::SeqCst) {
        let mut got = false;
        for (i, input) in inputs.iter_mut().enumerate() {
            loop {
                match input.rx.try_recv() {
                    Ok(chunk) => {
                        got = true;
                        take(i, &chunk, &mut phrasers, &mut writers, &mut batches);
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        closed[i] = true;
                        break;
                    }
                }
            }
        }
        if closed.iter().all(|c| *c) {
            break;
        }
        if last_level.elapsed() >= Duration::from_millis(250) {
            emit("level", levels());
            last_level = Instant::now();
        }
        if !got {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    // Whatever was still on its way, then the speech in progress.
    for (i, input) in inputs.iter_mut().enumerate() {
        while let Ok(chunk) = input.rx.try_recv() {
            take(i, &chunk, &mut phrasers, &mut writers, &mut batches);
        }
    }
    for (i, p) in phrasers.iter_mut().enumerate() {
        if let Some(u) = p.flush() {
            batches[i].add(u);
        }
        if let Some(job) = batches[i].take(speakers[i]) {
            let _ = job_tx.send(job);
        }
    }
    for w in writers.into_iter().flatten() {
        w.finish();
    }
    drop(job_tx);
    emit("state", json!({ "status": "transcribing" }));
    worker.await.map_err(err)?;
    Ok(())
}

// ---------- notes ----------

/// "[12:03] You: …" lines.
pub fn transcript_text(segs: &[Segment]) -> String {
    segs.iter()
        .map(|s| {
            let t = s.start as u64;
            format!("[{:02}:{:02}] {}: {}", t / 60, t % 60, s.speaker.label(), s.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn notes_schema() -> Value {
    let list = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "required": ["title", "summary", "topics", "key_points", "most_important", "decisions", "action_items"],
        "properties": {
            "title": { "type": "string" },
            "summary": { "type": "string" },
            "topics": list,
            "key_points": list,
            "most_important": list,
            "decisions": list,
            "action_items": { "type": "array", "items": {
                "type": "object",
                "required": ["task", "owner", "due"],
                "properties": { "task": { "type": "string" }, "owner": { "type": "string" }, "due": { "type": "string" } }
            } }
        }
    })
}

const NOTES_ASK: &str = "Write meeting notes from this transcript. \"You\" is the person who recorded it; \"Others\" are the \
other people on the call (they may be several people). Answer in JSON with:\n\
- title: a short name for the meeting (3-7 words)\n\
- summary: 2-4 sentences\n\
- topics: the general topics discussed\n\
- key_points: the key items, facts and updates\n\
- most_important: the 1-3 most important points\n\
- decisions: everything the group decided or agreed on, e.g. \"we decided to…\" or \"let's go with…\"\n\
- action_items: tasks someone agreed or was asked to do, each with task, owner and due. The owner is the person who \
will do it: a name if one was said (\"Jordan, please send…\" means Jordan), \"You\" if the recorder said they would do \
it, or \"\" if unclear. Due is as said, or \"\".\n\
Only include what was actually said. Write in the language of the transcript.";

/// Pulls the JSON object out of a reply that may have extra text around it.
fn parse_notes(reply: &str) -> Option<Notes> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    serde_json::from_str(&reply[start..=end]).ok()
}

async fn notes_from(ep: &Endpoint, text: &str) -> Result<Notes, String> {
    let messages = vec![
        json!({ "role": "system", "content": "You write accurate, concise meeting notes." }),
        json!({ "role": "user", "content": format!("{NOTES_ASK}\n\nTranscript:\n{text}") }),
    ];
    let schema = json!({ "response_format": { "type": "json_schema", "json_schema": { "name": "notes", "schema": notes_schema() } } });
    match chat::complete(ep, messages.clone(), schema, 3000).await {
        Ok(reply) => {
            if let Some(n) = parse_notes(&reply) {
                return Ok(n);
            }
        }
        Err(e) => eprintln!("notes with a schema failed, retrying without: {e}"),
    }
    // Some models or templates don't take a schema; ask plainly.
    let reply = chat::complete(ep, messages, json!({}), 3000).await?;
    parse_notes(&reply).ok_or_else(|| "The model's notes couldn't be read. Try “Write notes again”.".to_string())
}

/// Notes for a whole meeting. Long transcripts are summarized in parts
/// first, then combined, so any meeting length fits the model.
pub async fn write_notes(ep: &Endpoint, segs: &[Segment]) -> Result<Notes, String> {
    let text = transcript_text(segs);
    if text.trim().is_empty() {
        return Err("Nothing was said, so there's nothing to write up.".into());
    }
    // About 3.5 characters per token; leave room for the instructions and answer.
    let budget_chars = ((ep.ctx as usize).saturating_sub(4500) as f64 * 3.5 * 0.8) as usize;
    if text.len() <= budget_chars.max(4000) {
        return notes_from(ep, &text).await;
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if current.len() + line.len() > budget_chars.max(4000) && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        parts.push(current);
    }
    let mut digest = String::new();
    for (i, part) in parts.iter().enumerate() {
        let messages = vec![
            json!({ "role": "system", "content": "You take accurate, detailed notes." }),
            json!({ "role": "user", "content": format!(
                "This is part {} of {} of a meeting transcript. Write detailed bullet-point notes of everything discussed: \
                 facts, updates, decisions, and every task someone agreed to do (with who and when). Keep the speakers' \
                 labels where they matter.\n\n{part}", i + 1, parts.len()) }),
        ];
        let notes = chat::complete(ep, messages, json!({}), 1500).await?;
        digest.push_str(&format!("Part {} notes:\n{notes}\n\n", i + 1));
    }
    notes_from(ep, &digest).await
}

pub fn notes_markdown(m: &Meeting, segs: Option<&[Segment]>) -> String {
    let when = chrono::DateTime::from_timestamp_millis(m.started_at)
        .map(|t| t.with_timezone(&chrono::Local).format("%A, %B %-d, %Y at %-I:%M %p").to_string())
        .unwrap_or_default();
    let mut out = format!("# {}\n\n_{when}_\n\n", m.data.title);
    if let Some(n) = &m.data.notes {
        out.push_str(&format!("## Summary\n\n{}\n\n", n.summary));
        let section = |out: &mut String, title: &str, items: &[String]| {
            if !items.is_empty() {
                out.push_str(&format!("## {title}\n\n"));
                for i in items {
                    out.push_str(&format!("- {i}\n"));
                }
                out.push('\n');
            }
        };
        section(&mut out, "Most important", &n.most_important);
        if !n.action_items.is_empty() {
            out.push_str("## Action items\n\n");
            for a in &n.action_items {
                let mut line = format!("- [{}] {}", if a.done { "x" } else { " " }, a.task);
                if !a.owner.is_empty() {
                    line.push_str(&format!(" — {}", a.owner));
                }
                if !a.due.is_empty() {
                    line.push_str(&format!(" (due {})", a.due));
                }
                out.push_str(&line);
                out.push('\n');
            }
            out.push('\n');
        }
        section(&mut out, "Decisions", &n.decisions);
        section(&mut out, "Key points", &n.key_points);
        section(&mut out, "Topics", &n.topics);
    }
    if let Some(segs) = segs {
        out.push_str("## Transcript\n\n");
        for s in segs {
            let t = s.start as u64;
            out.push_str(&format!("**[{:02}:{:02}] {}:** {}\n\n", t / 60, t % 60, s.speaker.label(), s.text));
        }
    }
    out
}

/// Writes the notes for a finished recording and marks it done.
pub async fn finish(state: &Arc<AppState>, cipher: &Cipher, id: &str, emit: &Emit) -> Result<(), String> {
    set_status(&state.db.lock().unwrap(), id, "summarizing")?;
    emit("state", json!({ "status": "summarizing" }));
    let segs = segments(&state.db.lock().unwrap(), cipher, id);
    let result = async {
        let (ep, _) = crate::background_endpoint(state).await?;
        write_notes(&ep, &segs).await
    }
    .await;
    let conn = state.db.lock().unwrap();
    let mut m = load(&conn, cipher, id).ok_or("The meeting was deleted.")?;
    match result {
        Ok(notes) => {
            if !m.data.titled_by_user && !notes.title.trim().is_empty() {
                m.data.title = notes.title.trim().to_string();
            }
            m.data.notes = Some(notes);
            m.data.error = None;
        }
        Err(e) => m.data.error = Some(e),
    }
    save_data(&conn, cipher, id, &m.data)?;
    set_status(&conn, id, "done")?;
    db::log_action(&conn, "meeting", "Wrote notes for a meeting");
    Ok(())
}

// ---------- live session ----------

fn live() -> &'static Mutex<Option<(String, Arc<AtomicBool>)>> {
    static L: OnceLock<Mutex<Option<(String, Arc<AtomicBool>)>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

#[derive(Debug, Clone, Deserialize)]
pub struct StartOptions {
    pub title: Option<String>,
    pub mic: bool,
    pub system: bool,
    pub translate_to: Option<String>,
}

#[tauri::command]
pub async fn start_meeting(app: AppHandle, state: AppStateRef<'_>, options: StartOptions) -> Result<Meeting, String> {
    let cipher = state.cipher()?;
    if !options.mic && !options.system {
        return Err("Choose at least one thing to record.".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut l = live().lock().unwrap();
        if l.is_some() {
            return Err("A meeting is already being recorded.".into());
        }
        *l = Some((String::new(), stop.clone()));
    }
    let settings = speech::voice_settings(&state.db.lock().unwrap());
    let title = options.title.clone().filter(|t| !t.trim().is_empty());
    let data = MeetingData {
        titled_by_user: title.is_some(),
        title: title.unwrap_or_else(|| format!("Meeting · {}", chrono::Local::now().format("%b %-d, %-I:%M %p"))),
        translate_to: options.translate_to.clone().filter(|l| !l.is_empty()),
        has_audio: settings.keep_meeting_audio,
        ..Default::default()
    };
    let meeting = match create(&state.db.lock().unwrap(), &cipher, &data) {
        Ok(m) => m,
        Err(e) => {
            *live().lock().unwrap() = None;
            return Err(e);
        }
    };
    *live().lock().unwrap() = Some((meeting.id.clone(), stop.clone()));
    state.log("meeting", "Started recording a meeting");

    let state = state.inner().clone();
    let id = meeting.id.clone();
    tauri::async_runtime::spawn(async move {
        let emit: Emit = {
            let (app, id) = (app.clone(), id.clone());
            Arc::new(move |kind: &str, mut payload: Value| {
                payload["meeting_id"] = json!(id);
                payload["kind"] = json!(kind);
                app.emit("meeting", payload).ok();
            })
        };
        let _hold = speech::hold(&state);
        let result: Result<(), String> = async {
            emit("state", json!({ "status": "loading" }));
            let ep = speech::endpoint(&state, speech::Use::Accurate).await?;
            let mut inputs = Vec::new();
            let mut captures: Vec<(Speaker, Capture)> = Vec::new();
            if options.mic {
                let (tx, rx) = mpsc::unbounded_channel();
                captures.push((Speaker::You, Capture::start(Source::Mic { device: settings.mic.clone(), voice: settings.voice_processing }, tx)?));
                inputs.push(Input { speaker: Speaker::You, rx });
            }
            if options.system {
                let (tx, rx) = mpsc::unbounded_channel();
                captures.push((Speaker::Others, Capture::start(Source::System { device: None }, tx)?));
                inputs.push(Input { speaker: Speaker::Others, rx });
            }
            emit("state", json!({ "status": "recording" }));
            let captures = Arc::new(Mutex::new(captures));
            let caps = captures.clone();
            let levels = move || {
                let c = caps.lock().unwrap();
                let get = |sp: Speaker| c.iter().find(|(s, _)| *s == sp).map(|(_, cap)| cap.level());
                json!({ "you": get(Speaker::You), "others": get(Speaker::Others) })
            };
            // Stops the devices the moment the user ends the meeting.
            let stopper = {
                let (stop, captures) = (stop.clone(), captures.clone());
                tauri::async_runtime::spawn(async move {
                    while !stop.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    captures.lock().unwrap().clear();
                })
            };
            let r = record(&state, &cipher, &id, ep, inputs, settings.keep_meeting_audio, data.translate_to.clone(), &stop, &levels, emit.clone()).await;
            stop.store(true, Ordering::SeqCst);
            let _ = stopper.await;
            r?;
            {
                let conn = state.db.lock().unwrap();
                conn.execute("UPDATE meetings SET ended_at = ?2 WHERE id = ?1", params![id, db::now_ms()]).map_err(err)?;
            }
            *live().lock().unwrap() = None;
            finish(&state, &cipher, &id, &emit).await
        }
        .await;
        *live().lock().unwrap() = None;
        match result {
            Ok(()) => emit("done", json!({})),
            Err(e) => {
                let conn = state.db.lock().unwrap();
                let _ = conn.execute("UPDATE meetings SET ended_at = COALESCE(ended_at, ?2) WHERE id = ?1", params![id, db::now_ms()]);
                if let Some(mut m) = load(&conn, &cipher, &id) {
                    m.data.error = Some(e.clone());
                    let _ = save_data(&conn, &cipher, &id, &m.data);
                }
                let _ = set_status(&conn, &id, "failed");
                emit("error", json!({ "error": e }));
            }
        }
    });
    Ok(meeting)
}

#[tauri::command]
pub fn stop_meeting(state: AppStateRef) -> Result<(), String> {
    if let Some((_, stop)) = live().lock().unwrap().as_ref() {
        stop.store(true, Ordering::SeqCst);
        state.log("meeting", "Stopped recording a meeting");
    }
    Ok(())
}

/// The meeting being recorded right now, if any.
#[tauri::command]
pub fn live_meeting() -> Option<String> {
    live().lock().unwrap().as_ref().map(|(id, _)| id.clone()).filter(|id| !id.is_empty())
}

// ---------- library ----------

#[derive(Serialize)]
pub struct MeetingDetail {
    #[serde(flatten)]
    meeting: Meeting,
    segments: Vec<Segment>,
}

#[tauri::command]
pub fn list_meetings(state: AppStateRef) -> Result<Vec<Meeting>, String> {
    let c = state.cipher()?;
    Ok(list(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
pub fn get_meeting(state: AppStateRef, id: String) -> Result<MeetingDetail, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let meeting = load(&conn, &c, &id).ok_or("That meeting no longer exists.")?;
    Ok(MeetingDetail { segments: segments(&conn, &c, &id), meeting })
}

#[tauri::command]
pub fn rename_meeting(state: AppStateRef, id: String, title: String) -> Result<(), String> {
    let c = state.cipher()?;
    let title = title.trim();
    if title.is_empty() {
        return Err("A meeting needs a name.".into());
    }
    let conn = state.db.lock().unwrap();
    let mut m = load(&conn, &c, &id).ok_or("That meeting no longer exists.")?;
    m.data.title = title.to_string();
    m.data.titled_by_user = true;
    save_data(&conn, &c, &id, &m.data)
}

#[tauri::command]
pub fn set_action_done(state: AppStateRef, id: String, index: usize, done: bool) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut m = load(&conn, &c, &id).ok_or("That meeting no longer exists.")?;
    let item = m.data.notes.as_mut().and_then(|n| n.action_items.get_mut(index)).ok_or("No such action item.")?;
    item.done = done;
    save_data(&conn, &c, &id, &m.data)
}

#[tauri::command]
pub fn delete_meeting(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    if live_meeting().as_deref() == Some(id.as_str()) {
        return Err("Stop the recording first.".into());
    }
    let dir = audio_dir(&state, &id);
    if dir.starts_with(state.paths.data.join("meetings")) && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the recording: {e}"))?;
    }
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM meetings WHERE id = ?1", [&id]).map_err(err)?;
    db::log_action(&conn, "meeting", "Deleted a meeting, its transcript and its recording");
    Ok(())
}

#[tauri::command]
pub fn delete_meeting_audio(state: AppStateRef, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    let dir = audio_dir(&state, &id);
    if dir.starts_with(state.paths.data.join("meetings")) && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the recording: {e}"))?;
    }
    let conn = state.db.lock().unwrap();
    let mut m = load(&conn, &c, &id).ok_or("That meeting no longer exists.")?;
    m.data.has_audio = false;
    save_data(&conn, &c, &id, &m.data)?;
    db::log_action(&conn, "meeting", "Deleted a meeting's recording (kept the transcript)");
    Ok(())
}

#[tauri::command]
pub async fn rewrite_notes(app: AppHandle, state: AppStateRef<'_>, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    let state = state.inner().clone();
    let emit: Emit = {
        let id = id.clone();
        Arc::new(move |kind: &str, mut payload: Value| {
            payload["meeting_id"] = json!(id);
            payload["kind"] = json!(kind);
            app.emit("meeting", payload).ok();
        })
    };
    let r = finish(&state, &c, &id, &emit).await;
    emit("done", json!({}));
    r
}

/// A WAV of both channels between two times, for replaying part of a meeting.
#[tauri::command]
pub fn meeting_clip(state: AppStateRef, id: String, start: f64, end: f64) -> Result<tauri::ipc::Response, String> {
    let c = state.cipher()?;
    let end = end.min(start + 120.0);
    Ok(tauri::ipc::Response::new(clip(&audio_dir(&state, &id), &c, start, end)))
}

/// Saves the notes and transcript as a Markdown file the user chose.
#[tauri::command]
pub fn export_meeting(state: AppStateRef, id: String, path: String, transcript: bool) -> Result<(), String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let m = load(&conn, &c, &id).ok_or("That meeting no longer exists.")?;
    let segs = segments(&conn, &c, &id);
    std::fs::write(&path, notes_markdown(&m, transcript.then_some(segs.as_slice()))).map_err(|e| format!("Couldn't save the file: {e}"))?;
    db::log_action(&conn, "meeting", "Exported meeting notes to a file");
    Ok(())
}

/// Starts a chat about a meeting, seeded with its notes; the assistant can
/// read the full transcript with its meeting tools.
#[tauri::command]
pub fn ask_about_meeting(state: AppStateRef, id: String) -> Result<String, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let m = load(&conn, &c, &id).ok_or("That meeting no longer exists.")?;
    let chat = db::create_chat(&conn, &c, db::settings(&conn).default_model)?;
    db::set_chat_title(&conn, &c, &chat.id, &format!("About: {}", m.data.title))?;
    let mut intro = format!("Let's talk about **{}** (meeting id `{}`).\n\n", m.data.title, m.id);
    intro.push_str(&notes_markdown(&m, None).lines().skip(1).collect::<Vec<_>>().join("\n"));
    intro.push_str("\n\nAsk me anything about it. I can read the full transcript.");
    db::add_message(&conn, &c, &db::Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat.id.clone(),
        role: "assistant".into(),
        content: intro,
        created_at: db::now_ms(),
        meta: Some(json!({ "meeting_id": m.id })),
        ..Default::default()
    })?;
    Ok(chat.id)
}

/// Finds meetings by words in their title, notes or transcript.
pub fn search(conn: &Connection, c: &Cipher, query: &str, limit: usize) -> Vec<(Meeting, Vec<String>)> {
    let terms = words(query);
    let mut hits = Vec::new();
    for m in list(conn, c) {
        let segs = segments(conn, c, &m.id);
        let notes = m.data.notes.as_ref().map(|n| serde_json::to_string(n).unwrap_or_default()).unwrap_or_default();
        let hay = format!("{} {}", m.data.title, notes).to_lowercase();
        let mut score = terms.iter().filter(|t| hay.contains(t.as_str())).count() * 3;
        let mut lines = Vec::new();
        for s in &segs {
            let lower = s.text.to_lowercase();
            let n = terms.iter().filter(|t| lower.contains(t.as_str())).count();
            if n > 0 {
                score += n;
                if lines.len() < 3 {
                    let t = s.start as u64;
                    lines.push(format!("[{:02}:{:02}] {}: {}", t / 60, t % 60, s.speaker.label(), s.text));
                }
            }
        }
        if score > 0 || terms.is_empty() {
            hits.push((score, m, lines));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.started_at.cmp(&a.1.started_at)));
    hits.into_iter().take(limit).map(|(_, m, l)| (m, l)).collect()
}

#[derive(Serialize)]
pub struct SearchHit {
    meeting: Meeting,
    lines: Vec<String>,
}

#[tauri::command]
pub fn search_meetings(state: AppStateRef, query: String) -> Result<Vec<SearchHit>, String> {
    let c = state.cipher()?;
    Ok(search(&state.db.lock().unwrap(), &c, &query, 50).into_iter().map(|(meeting, lines)| SearchHit { meeting, lines }).collect())
}

pub fn count(conn: &Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM meetings", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Protector, Vault};

    struct Plain;
    impl Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }

    fn setup() -> (PathBuf, Connection, Cipher) {
        let d = std::env::temp_dir().join(format!("sulcusai-mt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let conn = db::open(&d.join("t.db")).unwrap();
        let c = Vault::open(&d.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        (d, conn, c)
    }

    #[test]
    fn batched_phrases_map_back_to_meeting_time() {
        let u = |start_secs: f64, secs: f64| vad::Utterance {
            start: (start_secs * 16_000.0) as u64,
            samples: vec![0.1; (secs * 16_000.0) as usize],
        };
        let mut b = Batch::default();
        b.add(u(10.0, 2.0));
        b.add(u(30.0, 3.0));
        assert!((b.secs() - 5.5).abs() < 1e-9, "two phrases and one short gap");
        let job = b.take(Speaker::Others).unwrap();
        assert!(b.take(Speaker::Others).is_none());
        assert!((job.real_time(0.5) - 10.5).abs() < 1e-9);
        // In the gap or past a piece's end, times stick to that piece.
        assert!((job.real_time(2.2) - 12.0).abs() < 1e-9);
        assert!((job.real_time(3.0) - 30.5).abs() < 1e-9);
    }

    #[test]
    fn broken_lines_are_rejoined_into_sentences() {
        let l = |a: f64, b: f64, t: &str| (a, b, t.to_string());
        let out = whole_sentences(vec![
            l(8.9, 11.9, "Jordan, please send the budget by"),
            l(11.9, 12.5, " Friday."),
            l(15.0, 17.0, "Sounds good, I'll book the"),
            l(19.5, 20.0, "venue."),
        ]);
        assert_eq!(out.len(), 3, "a long pause keeps lines apart");
        assert_eq!(out[0].2, "Jordan, please send the budget by Friday.");
        assert_eq!(out[0].1, 12.5);
    }

    #[test]
    fn echo_of_the_call_is_recognized() {
        assert!(is_echo("we should ship on Thursday", "We should ship it on Thursday."));
        assert!(!is_echo("I disagree, Friday is better", "We should ship it on Thursday."));
        assert!(!is_echo("yes", "yes"), "single words are too short to judge");
    }

    #[test]
    fn meetings_and_segments_are_stored_encrypted() {
        let (_d, conn, c) = setup();
        let m = create(&conn, &c, &MeetingData { title: "Budget sync".into(), ..Default::default() }).unwrap();
        add_segment(&conn, &c, &m.id, Speaker::Others, 1.0, 3.0, "The budget is approved.").unwrap();
        add_segment(&conn, &c, &m.id, Speaker::You, 0.2, 0.9, "Hi all.").unwrap();
        let raw: String = conn.query_row("SELECT text FROM meeting_segments LIMIT 1", [], |r| r.get(0)).unwrap();
        assert!(raw.starts_with("enc1:"));
        let raw: String = conn.query_row("SELECT data FROM meetings", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("Budget"));
        let segs = segments(&conn, &c, &m.id);
        assert_eq!(segs[0].text, "Hi all.", "ordered by time");
        assert_eq!(transcript_text(&segs), "[00:00] You: Hi all.\n[00:01] Others: The budget is approved.");
        let hits = search(&conn, &c, "budget approved", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1.len(), 1);
        // Deleting the meeting deletes its transcript.
        conn.execute("DELETE FROM meetings WHERE id = ?1", [&m.id]).unwrap();
        assert!(segments(&conn, &c, &m.id).is_empty());
    }

    #[test]
    fn audio_chunks_round_trip_across_a_boundary() {
        let (d, _conn, c) = setup();
        let mut w = AudioWriter::new(&d, Speaker::You, c.clone());
        // 40 s of a ramp, written in uneven pieces.
        let total = 40 * audio::RATE as usize;
        let signal: Vec<f32> = (0..total).map(|i| ((i % 1000) as f32 / 1000.0) - 0.5).collect();
        for piece in signal.chunks(7_777) {
            w.push(piece);
        }
        w.finish();
        let got = read_channel(&d, Speaker::You, &c, 29.0, 31.0);
        assert_eq!(got.len(), 2 * audio::RATE as usize);
        let want = &signal[29 * 16_000..31 * 16_000];
        assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3));
        // The stored files aren't readable audio.
        let raw = std::fs::read(d.join("you-00000.enc")).unwrap();
        assert!(raw.len() > CHUNK * 2);
        // Mixed clip of a channel that wasn't recorded is just the other one.
        let wav = clip(&d, &c, 0.0, 1.0);
        assert_eq!(audio::wav_decode(&wav).unwrap().samples.len(), audio::RATE as usize);
    }

    #[test]
    fn notes_are_parsed_from_a_wordy_reply() {
        let reply = "Here you go:\n```json\n{\"title\":\"Launch\",\"summary\":\"s\",\"topics\":[],\"key_points\":[],\"most_important\":[\"ship\"],\"decisions\":[],\"action_items\":[{\"task\":\"Send budget\",\"owner\":\"Jordan\",\"due\":\"Friday\"}]}\n```";
        let n = parse_notes(reply).unwrap();
        assert_eq!(n.action_items[0].owner, "Jordan");
        assert!(!n.action_items[0].done);
    }

    #[test]
    fn markdown_export_has_notes_and_transcript() {
        let m = Meeting {
            id: "m".into(),
            started_at: 0,
            ended_at: None,
            status: "done".into(),
            data: MeetingData {
                title: "Launch".into(),
                notes: Some(Notes {
                    summary: "We set a date.".into(),
                    action_items: vec![ActionItem { task: "Send budget".into(), owner: "Jordan".into(), due: "Friday".into(), done: true }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        };
        let segs = vec![Segment { id: 1, speaker: Speaker::Others, start: 65.0, end: 70.0, text: "Thursday works.".into(), translation: None }];
        let md = notes_markdown(&m, Some(&segs));
        assert!(md.starts_with("# Launch"));
        assert!(md.contains("- [x] Send budget — Jordan (due Friday)"));
        assert!(md.contains("**[01:05] Others:** Thursday works."));
    }
}

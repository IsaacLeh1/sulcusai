// SPDX-License-Identifier: AGPL-3.0-only
//! The Media index on disk: one row per file, everything but the id and
//! when it was added encrypted, since transcripts are what people said.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::index::{self, Chapter, Segment};
use crate::crypto::Cipher;
use crate::AppState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub path: String,
    pub title: String,
    pub seconds: f64,
    pub added: i64,
    pub segments: Vec<Segment>,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub path: String,
    pub title: String,
    pub seconds: f64,
    pub added: i64,
    pub chapters: usize,
    pub speakers: Vec<(String, f64)>,
}

fn table(conn: &Connection) -> Result<(), String> {
    conn.execute("CREATE TABLE IF NOT EXISTS media_index (id TEXT PRIMARY KEY, added INTEGER NOT NULL, data TEXT NOT NULL)", []).map(|_| ()).map_err(|e| e.to_string())
}

pub fn put(conn: &Connection, c: &Cipher, e: &Entry) -> Result<(), String> {
    table(conn)?;
    let data = c.encrypt(&serde_json::to_string(e).map_err(|e| e.to_string())?);
    conn.execute("INSERT OR REPLACE INTO media_index (id, added, data) VALUES (?1, ?2, ?3)", params![e.id, e.added, data]).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn all(conn: &Connection, c: &Cipher) -> Vec<Entry> {
    if table(conn).is_err() {
        return vec![];
    }
    let Ok(mut st) = conn.prepare("SELECT data FROM media_index ORDER BY added DESC") else { return vec![] };
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map(|r| r.flatten().collect::<Vec<_>>()).unwrap_or_default();
    rows.iter().filter_map(|d| c.decrypt(d).ok()).filter_map(|s| serde_json::from_str(&s).ok()).collect()
}

pub fn get(conn: &Connection, c: &Cipher, id: &str) -> Option<Entry> {
    all(conn, c).into_iter().find(|e| e.id == id)
}

pub fn remove(conn: &Connection, id: &str) -> Result<(), String> {
    table(conn)?;
    conn.execute("DELETE FROM media_index WHERE id = ?1", [id]).map(|_| ()).map_err(|e| e.to_string())
}

pub fn summary(e: &Entry) -> Summary {
    Summary { id: e.id.clone(), path: e.path.clone(), title: e.title.clone(), seconds: e.seconds, added: e.added, chapters: e.chapters.len(), speakers: index::speakers(&e.segments) }
}

/// The id is the file's path, so adding a file again replaces its entry.
pub fn id_for(path: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(path.to_lowercase().as_bytes())[..12])
}

/// Speaker labels from voice prints, one per segment; plain numbers by
/// first appearance, which the person can rename.
pub fn label(segs: &mut [Segment], groups: &[u32]) {
    for (s, g) in segs.iter_mut().zip(groups) {
        s.speaker = Some(format!("Speaker {}", g + 1));
    }
}

/// Reads the sound of any file FFmpeg can open as 16 kHz mono.
async fn sound(state: &Arc<AppState>, file: &Path, cancel: &AtomicBool) -> Result<(Vec<f32>, f64), String> {
    let (ffmpeg, ffprobe) = crate::media::ffmpeg::ensure(state, cancel, &|_, _| {}).await?;
    let probe = crate::media::ffmpeg::probe(&ffprobe, file).await?;
    if !probe.audio {
        return Err("This file has no sound to transcribe.".into());
    }
    let raw = std::env::temp_dir().join(format!("sulcusai-medialib-{}.f32", uuid::Uuid::new_v4().simple()));
    let args: Vec<std::ffi::OsString> =
        ["-i".into(), file.as_os_str().to_owned()].into_iter().chain(["-vn", "-ac", "1", "-ar", "16000", "-f", "f32le"].map(Into::into)).chain([raw.as_os_str().to_owned()]).collect();
    let ran = crate::media::ffmpeg::run(state, &ffmpeg, args, probe.seconds, cancel, &|_| {}).await;
    let bytes = std::fs::read(&raw);
    let _ = std::fs::remove_file(&raw);
    ran?;
    let bytes = bytes.map_err(|e| format!("FFmpeg didn't read the sound: {e}"))?;
    Ok((bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect(), probe.seconds))
}

/// Transcribes a file, tells speakers apart when the speaker model is
/// installed, cuts chapters and stores it.
pub async fn add(state: &Arc<AppState>, path: &str, cancel: &AtomicBool) -> Result<Entry, String> {
    let file = Path::new(path);
    if !file.is_file() {
        return Err("That file isn't there.".into());
    }
    let (samples, seconds) = sound(state, file, cancel).await?;
    let ep = crate::speech::endpoint(state, crate::speech::Use::Accurate).await.map_err(|e| format!("The Media index needs a speech model. {e}"))?;
    let mut segs: Vec<Segment> = Vec::new();
    for (a, b) in pieces(samples.len()) {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Stopped.".into());
        }
        let heard = crate::speech::clean(crate::speech::transcribe(&ep, &samples[a..b], &crate::speech::Options { language: Some("auto".into()), ..Default::default() }).await?);
        let off = a as f64 / RATE as f64;
        segs.extend(heard.segments.into_iter().map(|s| Segment { start: s.start + off, end: s.end + off, speaker: None, text: s.text }));
    }
    if crate::diarize::available(state) && segs.len() > 1 {
        let prints: Vec<Vec<f32>> = segs
            .iter()
            .filter_map(|s| {
                let a = ((s.start * 16000.0) as usize).min(samples.len());
                let b = ((s.end * 16000.0) as usize).clamp(a, samples.len());
                crate::diarize::embed(state, &samples[a..b]).ok()
            })
            .collect();
        if prints.len() == segs.len() {
            label(&mut segs, &crate::diarize::regroup(&prints));
        }
    }
    let entry = Entry {
        id: id_for(path),
        path: path.to_string(),
        title: file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        seconds,
        added: chrono::Utc::now().timestamp(),
        chapters: index::chapters(&segs, 2.5, 120.0),
        segments: segs,
    };
    let c = state.work_cipher()?;
    put(&state.db.lock().unwrap(), &c, &entry)?;
    crate::files::media_transcript(&state.paths.data, &entry.path, &transcript_text(&entry.segments));
    Ok(entry)
}

const RATE: usize = 16000;
const PIECE: usize = 30 * RATE;

/// Whisper hears 30 s at a time, and one request for an hour-long file
/// can time out or run out of memory, so long files go in 30 s pieces.
fn pieces(len: usize) -> Vec<(usize, usize)> {
    (0..len).step_by(PIECE).map(|a| (a, (a + PIECE).min(len))).collect()
}

/// The words for Find: who said it and what, one line each.
pub fn transcript_text(segs: &[Segment]) -> String {
    segs.iter().map(|s| match &s.speaker { Some(sp) => format!("{sp}: {}", s.text), None => s.text.clone() }).collect::<Vec<_>>().join("
")
}

/// Renames a speaker in one file, e.g. "Speaker 2" to "Ana".
pub fn rename_speaker(e: &mut Entry, from: &str, to: &str) {
    let to = to.trim();
    if to.is_empty() {
        return;
    }
    for s in &mut e.segments {
        if s.speaker.as_deref() == Some(from) {
            s.speaker = Some(to.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_sound_goes_in_30_second_pieces() {
        assert!(pieces(0).is_empty());
        assert_eq!(pieces(100), vec![(0, 100)]);
        assert_eq!(pieces(PIECE * 2 + 5), vec![(0, PIECE), (PIECE, PIECE * 2), (PIECE * 2, PIECE * 2 + 5)]);
    }

    fn entry(path: &str) -> Entry {
        Entry {
            id: id_for(path),
            path: path.into(),
            title: "t".into(),
            seconds: 3.0,
            added: 5,
            segments: vec![Segment { start: 0.0, end: 1.0, speaker: None, text: "hi".into() }, Segment { start: 1.0, end: 3.0, speaker: None, text: "there".into() }],
            chapters: vec![],
        }
    }

    #[test]
    fn stored_encrypted_and_replaced_by_path() {
        let conn = Connection::open_in_memory().unwrap();
        let c = Cipher::for_test();
        let mut e = entry("C:\\a\\Talk.mp4");
        put(&conn, &c, &e).unwrap();
        let raw: String = conn.query_row("SELECT data FROM media_index", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("there"));
        assert_eq!(id_for("c:\\A\\talk.MP4"), e.id);
        e.title = "new".into();
        put(&conn, &c, &e).unwrap();
        assert_eq!(all(&conn, &c).len(), 1);
        assert_eq!(get(&conn, &c, &e.id).unwrap().title, "new");
        remove(&conn, &e.id).unwrap();
        assert!(all(&conn, &c).is_empty());
    }

    #[test]
    fn speakers_labelled_and_renamed() {
        let mut e = entry("x");
        label(&mut e.segments, &[0, 1]);
        rename_speaker(&mut e, "Speaker 2", " Ana ");
        assert_eq!(e.segments[1].speaker.as_deref(), Some("Ana"));
        assert_eq!(summary(&e).speakers[0].0, "Ana");
        assert_eq!(transcript_text(&e.segments), "Speaker 1: hi
Ana: there");
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! The Studio's projects (Suno Studio's): tracks of clips the window edits
//! and plays (`src/studio`), kept here encrypted as the window writes them;
//! their sounds are gallery items (songs, parts made for them, recordings).
//! And what the window can't do itself: recording from the microphone,
//! and a track's notes read from its sound (Basic Pitch, or drum hits).

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::audio::{Capture, Source};
use crate::crypto::Cipher;
use crate::db;

const KEY: &str = "studio_projects";
const REC_RATE: u32 = 48_000;

/// Every project, the last changed first (as the window wrote them).
pub fn list(conn: &Connection, c: &Cipher) -> Vec<Value> {
    let mut all: Vec<Value> = db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.sort_by_key(|p| std::cmp::Reverse(p["updated_at"].as_i64().unwrap_or(0)));
    all
}

fn store(conn: &Connection, c: &Cipher, all: &[Value]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

/// Saves a project in place of the one with its id (new otherwise).
pub fn save(conn: &Connection, c: &Cipher, project: Value) -> Result<(), String> {
    let id = project["id"].as_str().filter(|s| !s.is_empty() && s.len() <= 64).ok_or("A project needs an id.")?.to_string();
    if serde_json::to_vec(&project).map_or(0, |b| b.len()) > 20_000_000 {
        return Err("That project is too big to keep.".into());
    }
    let mut all = list(conn, c);
    all.retain(|p| p["id"].as_str() != Some(&id));
    all.push(project);
    store(conn, c, &all)
}

pub fn remove(conn: &Connection, c: &Cipher, id: &str) -> Result<(), String> {
    let mut all = list(conn, c);
    all.retain(|p| p["id"].as_str() != Some(id));
    store(conn, c, &all)
}

// ---------- recording ----------

struct Recording {
    capture: Capture,
    samples: Arc<Mutex<Vec<f32>>>,
}

static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

/// Starts recording from the microphone picked in Settings › Voice (as it
/// sounds: no echo or noise removal), its level sent as "studio:level".
pub fn record_start(app: AppHandle, mic: Option<String>) -> Result<(), String> {
    let mut slot = RECORDING.lock().unwrap();
    if slot.is_some() {
        return Err("A recording is already running.".into());
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
    let capture = Capture::start_at(Source::Mic { device: mic, voice: false }, tx, REC_RATE)?;
    let samples = Arc::new(Mutex::new(Vec::new()));
    let into = samples.clone();
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
                let _ = app.emit("studio:level", serde_json::json!({ "level": level, "seconds": total as f32 / REC_RATE as f32 }));
            }
        }
    });
    *slot = Some(Recording { capture, samples });
    Ok(())
}

/// Stops recording: its samples (48 kHz mono), or why there are none.
pub fn record_stop() -> Result<Vec<f32>, String> {
    let rec = RECORDING.lock().unwrap().take().ok_or("Nothing is being recorded.")?;
    let device_error = rec.capture.error();
    drop(rec.capture);
    let samples = std::mem::take(&mut *rec.samples.lock().unwrap());
    if let Some(e) = device_error.filter(|_| samples.is_empty()) {
        return Err(e);
    }
    if samples.len() < (REC_RATE / 5) as usize {
        return Err("Nothing was recorded. Check the microphone in Settings › Voice.".into());
    }
    Ok(samples)
}

pub fn record_cancel() {
    RECORDING.lock().unwrap().take();
}

/// A recording as a 48 kHz stereo WAV (the same in both ears).
pub fn recording_wav(samples: &[f32]) -> Vec<u8> {
    super::own_voice::wav_stereo(samples, samples, REC_RATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_are_kept_and_replaced_by_id() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = Cipher::for_test();
        save(&conn, &c, serde_json::json!({ "id": "p1", "name": "One", "updated_at": 1 })).unwrap();
        save(&conn, &c, serde_json::json!({ "id": "p2", "name": "Two", "updated_at": 2 })).unwrap();
        save(&conn, &c, serde_json::json!({ "id": "p1", "name": "One again", "updated_at": 3 })).unwrap();
        let all = list(&conn, &c);
        assert_eq!(all.iter().map(|p| p["name"].as_str().unwrap()).collect::<Vec<_>>(), vec!["One again", "Two"]);
        assert!(save(&conn, &c, serde_json::json!({ "name": "no id" })).is_err());
        remove(&conn, &c, "p2").unwrap();
        assert_eq!(list(&conn, &c).len(), 1);
    }
}

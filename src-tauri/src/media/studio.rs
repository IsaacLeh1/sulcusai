// SPDX-License-Identifier: AGPL-3.0-only
//! The Studio's projects (Suno Studio's): tracks of clips the window edits
//! and plays (`src/studio`), kept here encrypted as the window writes them;
//! their sounds are gallery items (songs, parts made for them, recordings).
//! And what the window can't do itself: recording from the microphone,
//! and a track's notes read from its sound (Basic Pitch, or drum hits).

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde_json::{json, Value};
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

// ---------- the synth's saved sounds ----------

const PRESETS_KEY: &str = "studio_synth_presets";

/// The wavetable synth's sounds saved by name, in the order saved.
pub fn presets(conn: &Connection, c: &Cipher) -> Vec<Value> {
    db::get::<String>(conn, PRESETS_KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Saves a sound in place of the one with its name.
pub fn save_preset(conn: &Connection, c: &Cipher, preset: Value) -> Result<(), String> {
    let name = preset["name"].as_str().map(str::trim).filter(|s| !s.is_empty() && s.chars().count() <= 60).ok_or("A sound needs a name.")?.to_string();
    if serde_json::to_vec(&preset).map_or(0, |b| b.len()) > 200_000 {
        return Err("That sound is too big to keep.".into());
    }
    let mut all = presets(conn, c);
    if all.len() >= 500 && !all.iter().any(|p| p["name"].as_str() == Some(&name)) {
        return Err("That's as many sounds as can be kept; delete one first.".into());
    }
    all.retain(|p| p["name"].as_str() != Some(&name));
    all.push(preset);
    db::set(conn, PRESETS_KEY, &c.encrypt(&serde_json::to_string(&all).map_err(|e| e.to_string())?))
}

pub fn remove_preset(conn: &Connection, c: &Cipher, name: &str) -> Result<(), String> {
    let mut all = presets(conn, c);
    all.retain(|p| p["name"].as_str() != Some(name));
    db::set(conn, PRESETS_KEY, &c.encrypt(&serde_json::to_string(&all).map_err(|e| e.to_string())?))
}

// ---------- the chat bar ----------

/// What the chat model is told for the chat bar (Suno Studio's): the
/// actions it can take, as JSON, and the project as it is.
pub fn ask_messages(request: &str, summary: &str) -> Vec<Value> {
    let system = r#"You edit a song in a music app's song studio. Turn the person's request into a JSON array of actions, nothing else. Tracks are named as in the project. Actions:
{"do":"add_track","kind":"audio"|"midi","name":"...","instrument":0-127 (General MIDI) or 128 (drums) or "synth" (the wavetable synth), for midi; with "synth" also "preset" and "settings" as in the synth action}
{"do":"delete_track","track":"..."}
{"do":"rename_track","track":"...","name":"..."}
{"do":"mute"|"unmute"|"solo"|"unsolo","track":"..."}
{"do":"volume","track":"...","db":-40 to 6}
{"do":"pan","track":"...","value":-1 (left) to 1 (right)}
{"do":"add_effect","track":"...","effect":"eq"|"compressor"|"reverb"|"delay"|"distortion"|"gate","settings":{optional: eq low/mid/high dB -15..15; reverb size 0.3..6 s, mix 0..1; delay time s, feedback 0..0.9, mix 0..1; compressor threshold dB, ratio; distortion drive 0..1, mix; gate threshold dB}}
{"do":"clear_effects","track":"..."}
{"do":"instrument","track":"...","instrument":0-128}
{"do":"synth","track":"...","preset":"Init"|"808"|"Reese"|"Acid"|"Warm pad"|"Glass pad"|"Pluck"|"Bell pluck"|"Retro game"|"Supersaw lead"|"Sub bass"|"Wobble bass"|"Brass"|"Strings"|"Vocal pad"|"Organ"|"Keys","settings":{optional: "name"; "wavetable" "basic"|"pulse"|"analog"|"vocal"|"organ"|"bell"|"sync"|"bits"|"harmonics"|"phase"; "position" 0..1; "unison" 1..7; "detune" cents 0..100; "spread" 0..1; "osc2" a wavetable name or false; "osc2_octave" -3..3; "osc2_semitones"; "sub" 0..1; "noise" 0..1; "filter_type" "lowpass"|"highpass"|"bandpass"|"notch"; "cutoff" Hz; "resonance" 0..1; "filter_env" octaves -6..6; "drive" 0..1; "attack","decay","release" seconds; "sustain" 0..1; "glide" seconds; "mono" true|false; "legato" true|false; "octave" -3..3; "lfo_to" "position"|"cutoff"|"pitch"|"volume"|"pan"|"warp" with "lfo_rate" Hz or "lfo_beats" (beats a cycle) and "lfo_amount" -1..1}} puts a MIDI track on the wavetable synth: start from the closest preset, then settings to make the sound described
{"do":"transpose","track":"...","semitones":-12 to 12}
{"do":"fade","track":"...","in":seconds,"out":seconds}
{"do":"tempo","bpm":30-300}
{"do":"time_signature","beats":1-16}
{"do":"loop","from":seconds,"to":seconds}
{"do":"loop_off"}
{"do":"make_part","part":"vocals"|"backing_vocals"|"drums"|"bass"|"guitar"|"keyboard"|"percussion"|"strings"|"synth"|"fx"|"brass"|"woodwinds","description":"how it sounds","lyrics":"words, for vocals","from":seconds,"to":seconds,"takes":1-3}
{"do":"say","text":"..."} to answer, or to say what you can't do.
Use only these. Answer with the JSON array only."#;
    vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": format!("The project:\n{summary}\n\nThe request: {request}") }),
    ]
}

/// The actions in the chat model's answer: its JSON array (or an object
/// holding one), each with a "do"; anything else is left out.
pub fn actions_of(answer: &str) -> Vec<Value> {
    let text = answer.trim();
    let start = text.find('[');
    let end = text.rfind(']');
    let parsed: Option<Value> = match (start, end) {
        (Some(a), Some(b)) if b > a => serde_json::from_str(&text[a..=b]).ok(),
        _ => text.find('{').and_then(|a| serde_json::from_str::<Value>(&text[a..=text.rfind('}')?]).ok()),
    };
    let list = match parsed {
        Some(Value::Array(a)) => a,
        Some(Value::Object(o)) => o.get("actions").and_then(|a| a.as_array().cloned()).unwrap_or_else(|| vec![Value::Object(o)]),
        _ => Vec::new(),
    };
    list.into_iter().filter(|a| a["do"].as_str().is_some_and(|d| !d.is_empty())).take(30).collect()
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
    fn the_chat_models_actions_are_read() {
        let a = actions_of("Sure! [{\"do\":\"tempo\",\"bpm\":96},{\"do\":\"mute\",\"track\":\"Drums\"},{\"nope\":1}]");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0]["bpm"], 96);
        assert_eq!(actions_of("{\"actions\":[{\"do\":\"loop_off\"}]}").len(), 1);
        assert_eq!(actions_of("{\"do\":\"say\",\"text\":\"hi\"}")[0]["text"], "hi");
        assert!(actions_of("I can't do that.").is_empty());
    }

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

    #[test]
    fn synth_sounds_are_kept_by_name() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = Cipher::for_test();
        save_preset(&conn, &c, serde_json::json!({ "name": "Growl", "level": 0.5 })).unwrap();
        save_preset(&conn, &c, serde_json::json!({ "name": "Pad", "level": 0.4 })).unwrap();
        save_preset(&conn, &c, serde_json::json!({ "name": "Growl", "level": 0.7 })).unwrap();
        let all = presets(&conn, &c);
        assert_eq!(all.iter().map(|p| p["name"].as_str().unwrap()).collect::<Vec<_>>(), vec!["Pad", "Growl"]);
        assert_eq!(all[1]["level"], 0.7);
        assert!(save_preset(&conn, &c, serde_json::json!({ "name": "  " })).is_err());
        remove_preset(&conn, &c, "Pad").unwrap();
        assert_eq!(presets(&conn, &c).len(), 1);
    }
}

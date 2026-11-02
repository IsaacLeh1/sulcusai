// SPDX-License-Identifier: AGPL-3.0-only
//! Talking in another voice as you speak: the microphone is turned into one
//! of the voices in Studio › Your voice and played on the speakers, about a
//! second and a half behind. The singing converter runs on each three
//! quarters of a second with a little of what came before it, and the
//! pieces are cross-faded. It needs the graphics card: each piece takes
//! about half a second there, far longer on the processor.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::json;
use tauri::{AppHandle, Emitter};

use super::own_voice;
use super::svc::{self, Converter, Noise, Settings};
use crate::audio::{Capture, Player, Resampler};
use crate::AppStateRef;

/// New speech converted at a time: each run costs about the same whatever
/// its length (the speech encoder always reads 30 s), so a longer piece
/// leaves the card room to keep up.
pub(crate) const BLOCK: usize = svc::RATE as usize * 3 / 4;
/// What came before it, for context.
pub(crate) const CONTEXT: usize = svc::RATE as usize * 5 / 4;
/// Cross-faded between one piece and the next.
pub(crate) const FADE: usize = svc::RATE as usize / 25;
/// The person's voice prompt: shorter than a song's, so each step is quick.
const PROMPT: usize = svc::RATE as usize * 4;
/// Quieter than this is silence: nothing is converted.
const QUIET: f32 = 0.006;

static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

pub fn running() -> bool {
    RUNNING.lock().unwrap().is_some()
}

/// Joins `next` onto what was played: its first FADE samples blend from the
/// end of the last block, and its own last FADE are kept for the next one.
pub(crate) fn join(tail: &mut Vec<f32>, next: &[f32]) -> Vec<f32> {
    let n = next.len().saturating_sub(FADE);
    let mut out = next[..n].to_vec();
    for (i, v) in out.iter_mut().take(tail.len().min(FADE)).enumerate() {
        let g = i as f32 / FADE as f32;
        *v = *v * g + tail[i] * (1.0 - g);
    }
    *tail = next[n..].to_vec();
    out
}

#[tauri::command]
pub async fn live_voice_start(app: AppHandle, state: AppStateRef<'_>, voice_id: String, semitones: i32) -> Result<(), String> {
    let state = state.inner().clone();
    let c = state.work_cipher()?;
    let voice = own_voice::get(&state.db.lock().unwrap(), &c, &voice_id).ok_or("That voice is no longer here.")?;
    // Talking is copied best from the voice talking; singing when there's no reading.
    let reference = match own_voice::speech_reference(&state.paths, &c, &voice) {
        Ok((r, _)) => super::stems::resample(&r, own_voice::TTS_RATE, svc::RATE),
        Err(_) => own_voice::singing_reference(&state.paths, &c, &voice)?.0,
    };
    let reference: Vec<f32> = reference.into_iter().take(PROMPT).collect();
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut r = RUNNING.lock().unwrap();
        if r.is_some() {
            return Err("The voice changer is already on.".into());
        }
        *r = Some(stop.clone());
    }
    let emit = move |what: &str, extra: serde_json::Value| {
        let mut v = json!({ "state": what });
        if let (Some(o), serde_json::Value::Object(e)) = (v.as_object_mut(), extra) {
            o.extend(e);
        }
        app.emit("live_voice", v).ok();
    };
    emit("loading", json!({}));
    let ready = async {
        let never = AtomicBool::new(false);
        let models = own_voice::sing_parts(&state, &never, &|_, _| {}).await?;
        let picked = crate::natural::pick_card(&state, false).await.map_err(|e| format!("the voice changer needs a graphics card, and {e}"))?;
        if picked.needs_cuda {
            crate::natural::ensure_card_runtime(&state, true, &never, &|_, _| {}).await?;
        }
        let dll = crate::natural::ensure_runtime(&state, &never, &|_, _, _| {}).await?;
        Ok::<_, String>((models, dll, picked.card))
    }
    .await;
    let (models, dll, card) = match ready {
        Ok(v) => v,
        Err(e) => {
            RUNNING.lock().unwrap().take();
            emit("error", json!({ "error": e }));
            return Err(e);
        }
    };
    state.log("voice", &format!("The live voice changer started, in the voice “{}”", voice.name));
    let threads = state.limits().threads;
    let mic = crate::voice::mic(&state);
    std::thread::Builder::new()
        .name("live-voice".into())
        .spawn(move || {
            let result = run(&dll, &models, threads, card, &reference, mic, semitones, &stop, &emit);
            RUNNING.lock().unwrap().take();
            match result {
                Ok(()) => emit("stopped", json!({})),
                Err(e) => emit("error", json!({ "error": e })),
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run(
    dll: &std::path::Path,
    models: &std::path::Path,
    threads: usize,
    card: crate::natural::Card,
    reference: &[f32],
    mic: crate::audio::Source,
    semitones: i32,
    stop: &AtomicBool,
    emit: &(dyn Fn(&str, serde_json::Value) + Send + Sync),
) -> Result<(), String> {
    let mut conv = Converter::new(dll, models, threads, Some(card))?;
    let voice = conv.voice(reference)?;
    let settings = Settings { steps: 6, cfg: 0.7, semitones, seed: 7 };
    let mut noise = Noise::new(7);
    // Warmed up once, so the first words aren't held up by the pieces opening.
    conv.convert_live(&voice, &vec![0.0; CONTEXT + BLOCK + FADE], BLOCK + FADE, settings, &mut noise)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = Capture::start(mic, tx)?;
    let mut up = Resampler::new(crate::audio::RATE, svc::RATE);
    let player = Player::new();
    let mut heard: Vec<f32> = Vec::new();
    let mut fresh = 0usize;
    let mut tail: Vec<f32> = Vec::new();
    emit("on", json!({}));
    while !stop.load(Ordering::Relaxed) {
        let Some(chunk) = rx.blocking_recv() else { break };
        let more = up.process(&chunk);
        fresh += more.len();
        heard.extend(more);
        if fresh < BLOCK {
            continue;
        }
        fresh -= BLOCK;
        let keep = CONTEXT + BLOCK + FADE;
        if heard.len() > keep {
            heard.drain(..heard.len() - keep);
        }
        let started = Instant::now();
        let block = &heard[heard.len().saturating_sub(BLOCK)..];
        let out = if crate::audio::rms(block) < QUIET {
            vec![0.0; BLOCK + FADE]
        } else {
            let mut window = vec![0.0; keep.saturating_sub(heard.len())];
            window.extend_from_slice(&heard);
            conv.convert_live(&voice, &window, BLOCK + FADE, settings, &mut noise)?
        };
        let played = join(&mut tail, &out);
        player.play(svc::RATE, played.iter().map(|v| crate::audio::to_i16(*v)).collect());
        let took = started.elapsed().as_millis() as u64;
        emit("level", json!({ "level": crate::audio::rms(block), "took_ms": took, "behind": took as usize > BLOCK * 1000 / svc::RATE as usize }));
    }
    player.stop();
    let device_error = capture.error();
    drop(capture);
    device_error.map_or(Ok(()), Err)
}

#[tauri::command(async)]
pub fn live_voice_stop(state: AppStateRef) {
    if let Some(flag) = RUNNING.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Relaxed);
        state.log("voice", "The live voice changer stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_are_cross_faded_without_a_jump() {
        let mut tail = Vec::new();
        let first = join(&mut tail, &vec![1.0; BLOCK + FADE]);
        assert_eq!(first.len(), BLOCK);
        assert_eq!(tail.len(), FADE);
        let second = join(&mut tail, &vec![0.0; BLOCK + FADE]);
        // It starts where the first left off and fades down evenly.
        assert!((second[0] - 1.0).abs() < 1e-6);
        assert!(second.windows(2).take(FADE).all(|w| w[1] <= w[0] + 1e-6));
        assert!(second[FADE..].iter().all(|v| *v == 0.0));
    }
}

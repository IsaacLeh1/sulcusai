// SPDX-License-Identifier: AGPL-3.0-only
//! Live captions for whatever the PC is playing (a stream, a video, a
//! call): heard from the speakers, written as lines while it plays, and
//! kept so they can be saved as a subtitle file afterwards.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::Emitter;

use super::subs::Cue;
use crate::AppState;

static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
static CUES: Mutex<Vec<Cue>> = Mutex::new(Vec::new());

/// Where to cut the sound: at a quiet moment after at least `min` samples,
/// or at `max` whatever is playing, so a line never waits too long.
pub fn cut_at(buf: &[f32], rate: usize, min_s: f64, max_s: f64) -> Option<usize> {
    let (min, max) = ((min_s * rate as f64) as usize, (max_s * rate as f64) as usize);
    if buf.len() < min {
        return None;
    }
    let win = rate / 10;
    let mut i = min;
    while i + win <= buf.len().min(max) {
        if crate::audio::rms(&buf[i..i + win]) < 0.01 {
            return Some(i + win / 2);
        }
        i += win;
    }
    (buf.len() >= max).then_some(max)
}

pub fn running() -> bool {
    RUNNING.lock().unwrap().is_some()
}

pub fn cues() -> Vec<Cue> {
    CUES.lock().unwrap().clone()
}

pub fn stop() {
    if let Some(flag) = RUNNING.lock().unwrap().take() {
        flag.store(true, Ordering::Relaxed);
    }
}

pub async fn start(app: tauri::AppHandle, state: Arc<AppState>, translate: bool) -> Result<(), String> {
    if running() {
        return Ok(());
    }
    let ep = crate::speech::endpoint(&state, crate::speech::Use::Live).await.map_err(|e| format!("Live captions need a speech model. {e}"))?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = crate::audio::Capture::start(crate::audio::Source::System { device: None }, tx)?;
    let stop_flag = Arc::new(AtomicBool::new(false));
    *RUNNING.lock().unwrap() = Some(stop_flag.clone());
    CUES.lock().unwrap().clear();
    let _speech = crate::speech::hold(&state);
    tauri::async_runtime::spawn(async move {
        let _capture = capture;
        let _speech = _speech;
        let rate = crate::audio::RATE as usize;
        let mut buf: Vec<f32> = Vec::new();
        let mut at = 0.0f64;
        let mut prompt = String::new();
        let opts = |prompt: &str| crate::speech::Options { translate, prompt: Some(prompt.to_string()), ..Default::default() };
        while !stop_flag.load(Ordering::Relaxed) {
            match tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await {
                Ok(Some(chunk)) => buf.extend(chunk),
                Ok(None) => break,
                Err(_) => {}
            }
            let Some(cut) = cut_at(&buf, rate, 3.0, 8.0) else { continue };
            let part: Vec<f32> = buf.drain(..cut).collect();
            let len = part.len() as f64 / rate as f64;
            if crate::audio::rms(&part) > 0.005 {
                if let Ok(t) = crate::speech::transcribe(&ep, &part, &opts(&prompt)).await {
                    let text = crate::speech::clean(t).text;
                    if !text.is_empty() {
                        let cue = Cue { start: at, end: at + len, text: text.clone() };
                        CUES.lock().unwrap().push(cue.clone());
                        let _ = app.emit("media:caption", &cue);
                        prompt = text;
                    }
                }
            }
            at += len;
        }
        *RUNNING.lock().unwrap() = None;
        let _ = app.emit("media:caption-stopped", ());
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_at_quiet_or_at_most() {
        let rate = 100;
        let mut loud = vec![0.5f32; 500];
        assert_eq!(cut_at(&loud, rate, 3.0, 8.0), None);
        loud.extend(vec![0.0; 20]);
        loud.extend(vec![0.5; 100]);
        assert_eq!(cut_at(&loud, rate, 3.0, 8.0), Some(505));
        let all = vec![0.5f32; 900];
        assert_eq!(cut_at(&all, rate, 3.0, 8.0), Some(800));
        assert_eq!(cut_at(&vec![0.0; 310], rate, 3.0, 8.0), Some(305));
    }
}

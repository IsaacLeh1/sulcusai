// SPDX-License-Identifier: AGPL-3.0-only
//! Speaker labels: tells the other people in a call apart ("Speaker 1",
//! "Speaker 2") from the sound of their voices.
//!
//! Each transcript line's audio becomes an 80-band filterbank (computed the
//! way Kaldi does, which the model was trained on), then a voice "print"
//! from a WeSpeaker ResNet34 model on ONNX Runtime. Prints are grouped live,
//! and regrouped over the whole meeting at the end.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::catalog::License;
use crate::paths::Paths;
use crate::{db, download, net, AppState, AppStateRef};

/// Lines shorter than this don't carry enough voice to judge.
pub const MIN_SECS: f64 = 1.2;
/// Cosine similarity above which two prints are the same person.
pub const SAME_SPEAKER: f32 = 0.5;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpeakerModel {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub source: String,
    pub description: String,
    pub license: License,
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

fn model_path(paths: &Paths, m: &SpeakerModel) -> PathBuf {
    paths.models.join(&m.id).join(&m.file)
}

fn slot() -> &'static Mutex<Option<Session>> {
    static S: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The model file and the runtime are both present.
pub fn available(state: &AppState) -> bool {
    let m = &state.catalog.diarization;
    model_path(&state.paths, m).metadata().is_ok_and(|f| f.len() == m.size) && crate::natural::runtime_dll(&state.paths, &state.catalog.voices).is_some()
}

fn e(err: impl std::fmt::Display) -> String {
    format!("Speaker labels failed: {err}")
}

// ---------- features ----------

/// In-place radix-2 FFT (n a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

fn mel(f: f32) -> f32 {
    1127.0 * (1.0 + f / 700.0).ln()
}

/// Kaldi-style log mel filterbank: 25 ms frames every 10 ms, Hamming
/// window, pre-emphasis 0.97, 80 bands from 20 Hz to 8 kHz, then the mean
/// over time removed. Input: 16 kHz samples in -1..1.
pub fn fbank(samples: &[f32]) -> (usize, Vec<f32>) {
    const LEN: usize = 400;
    const SHIFT: usize = 160;
    const NFFT: usize = 512;
    const BINS: usize = 80;
    if samples.len() < LEN {
        return (0, Vec::new());
    }
    let frames = 1 + (samples.len() - LEN) / SHIFT;
    let window: Vec<f32> = (0..LEN).map(|i| 0.54 - 0.46 * (std::f32::consts::TAU * i as f32 / (LEN - 1) as f32).cos()).collect();
    // Triangular filters, evenly spaced on the mel scale.
    let (lo, hi) = (mel(20.0), mel(8000.0));
    let delta = (hi - lo) / (BINS + 1) as f32;
    let filters: Vec<Vec<(usize, f32)>> = (0..BINS)
        .map(|b| {
            let (left, center, right) = (lo + b as f32 * delta, lo + (b + 1) as f32 * delta, lo + (b + 2) as f32 * delta);
            (0..NFFT / 2)
                .filter_map(|i| {
                    let m = mel(i as f32 * 16_000.0 / NFFT as f32);
                    let w = if m > left && m < right {
                        if m <= center { (m - left) / (center - left) } else { (right - m) / (right - center) }
                    } else {
                        0.0
                    };
                    (w > 0.0).then_some((i, w))
                })
                .collect()
        })
        .collect();
    let mut out = vec![0.0f32; frames * BINS];
    let (mut re, mut im) = (vec![0.0f32; NFFT], vec![0.0f32; NFFT]);
    for f in 0..frames {
        // The model was trained on 16-bit sample values.
        let frame: Vec<f32> = samples[f * SHIFT..f * SHIFT + LEN].iter().map(|s| s * 32768.0).collect();
        let mean = frame.iter().sum::<f32>() / LEN as f32;
        re.iter_mut().for_each(|x| *x = 0.0);
        im.iter_mut().for_each(|x| *x = 0.0);
        for i in (0..LEN).rev() {
            let x = frame[i] - mean;
            let prev = if i > 0 { frame[i - 1] - mean } else { x };
            re[i] = (x - 0.97 * prev) * window[i];
        }
        fft(&mut re, &mut im);
        let power: Vec<f32> = (0..NFFT / 2).map(|i| re[i] * re[i] + im[i] * im[i]).collect();
        for (b, filt) in filters.iter().enumerate() {
            let energy: f32 = filt.iter().map(|(i, w)| power[*i] * w).sum();
            out[f * BINS + b] = energy.max(f32::EPSILON).ln();
        }
    }
    for b in 0..BINS {
        let mean = (0..frames).map(|f| out[f * BINS + b]).sum::<f32>() / frames as f32;
        (0..frames).for_each(|f| out[f * BINS + b] -= mean);
    }
    (frames, out)
}

/// A voice print for some speech (16 kHz), normalized to length 1.
pub fn embed(state: &AppState, samples: &[f32]) -> Result<Vec<f32>, String> {
    let m = &state.catalog.diarization;
    let (frames, feats) = fbank(samples);
    if frames < 50 {
        return Err("Too short to recognize a voice.".into());
    }
    let mut slot = slot().lock().unwrap();
    if slot.is_none() {
        let dll = crate::natural::runtime_dll(&state.paths, &state.catalog.voices).ok_or("ONNX Runtime isn't installed.")?;
        crate::natural::runtime_ready(&dll)?;
        let s = Session::builder().map_err(e)?.with_intra_threads(2).map_err(e)?.commit_from_file(model_path(&state.paths, m)).map_err(e)?;
        *slot = Some(s);
    }
    let session = slot.as_mut().unwrap();
    let input = Tensor::from_array(([1usize, frames, 80], feats)).map_err(e)?;
    let out = session.run(ort::inputs![input]).map_err(e)?;
    let v = out[0].try_extract_tensor::<f32>().map_err(e)?.1.to_vec();
    Ok(normalize(v))
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ---------- grouping ----------

/// Live grouping: each new print joins the closest speaker if it is close
/// enough, else starts a new one. Speakers are numbered from 1.
#[derive(Default)]
pub struct Live {
    centroids: Vec<(Vec<f32>, usize)>,
}

impl Live {
    pub fn assign(&mut self, v: &[f32]) -> u32 {
        let best = self.centroids.iter().enumerate().map(|(i, (c, _))| (i, cosine(c, v))).max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, sim)) if sim >= SAME_SPEAKER => {
                let (c, n) = &mut self.centroids[i];
                for (x, y) in c.iter_mut().zip(v) {
                    *x = (*x * *n as f32 + y) / (*n as f32 + 1.0);
                }
                *n += 1;
                *c = normalize(c.clone());
                i as u32 + 1
            }
            _ => {
                self.centroids.push((v.to_vec(), 1));
                self.centroids.len() as u32
            }
        }
    }
}

/// Regroups all prints of a meeting (average linkage), numbering speakers
/// by when they first spoke. Returns one label per print.
pub fn regroup(prints: &[Vec<f32>]) -> Vec<u32> {
    let mut clusters: Vec<Vec<usize>> = (0..prints.len()).map(|i| vec![i]).collect();
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for a in 0..clusters.len() {
            for b in a + 1..clusters.len() {
                let mut total = 0.0;
                for &i in &clusters[a] {
                    for &j in &clusters[b] {
                        total += cosine(&prints[i], &prints[j]);
                    }
                }
                let avg = total / (clusters[a].len() * clusters[b].len()) as f32;
                if best.is_none_or(|(_, _, s)| avg > s) {
                    best = Some((a, b, avg));
                }
            }
        }
        match best {
            Some((a, b, sim)) if sim >= SAME_SPEAKER => {
                let merged = clusters.remove(b);
                clusters[a].extend(merged);
            }
            _ => break,
        }
    }
    clusters.sort_by_key(|c| *c.iter().min().unwrap_or(&usize::MAX));
    let mut labels = vec![0u32; prints.len()];
    for (k, c) in clusters.iter().enumerate() {
        for &i in c {
            labels[i] = k as u32 + 1;
        }
    }
    labels
}

// ---------- install ----------

async fn install(state: &Arc<AppState>, cancel: &AtomicBool, emit: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<(), String> {
    emit("engine", 0, 0);
    crate::natural::ensure_runtime(state, cancel, emit).await?;
    let m = &state.catalog.diarization;
    let client = net::external_client(state.settings().connectivity, net::Purpose::ModelDownload, false)?;
    let dest = model_path(&state.paths, m);
    if !dest.exists() {
        state.log("network", &format!("Downloading {} ({}) from {}", m.name, crate::size_label(m.size), crate::host_of(&m.url)));
    }
    download::fetch_verified(&client, &m.url, &dest, m.size, &m.sha256, cancel, |r, t| emit("download", r, t)).await?;
    emit("benchmark", 0, 0);
    // Prove it runs: two seconds of a tone give some print.
    let tone: Vec<f32> = (0..32_000).map(|i| (i as f32 * 0.05).sin() * 0.2).collect();
    let s = state.clone();
    tokio::task::spawn_blocking(move || embed(&s, &tone)).await.map_err(|e| e.to_string())??;
    db::log_action(&state.db.lock().unwrap(), "model", &format!("Installed {}", m.name));
    Ok(())
}

#[cfg(test)]
pub async fn install_for_test(state: &Arc<AppState>) -> Result<(), String> {
    install(state, &AtomicBool::new(false), &|_, _, _| {}).await
}

#[derive(Serialize)]
pub struct SpeakerModelView {
    #[serde(flatten)]
    model: SpeakerModel,
    installed: bool,
    /// Download size including the shared runtime, if it isn't there yet.
    download: u64,
}

#[tauri::command]
pub fn speaker_model(state: AppStateRef) -> SpeakerModelView {
    let m = state.catalog.diarization.clone();
    let runtime = crate::natural::runtime_dll(&state.paths, &state.catalog.voices).is_none();
    let rt_size = state.catalog.voices.runtime.assets.get(crate::engine::cpu_key()).map_or(0, |a| a.size);
    SpeakerModelView { installed: available(&state), download: m.size + if runtime { rt_size } else { 0 }, model: m }
}

#[tauri::command]
pub fn install_speaker_model(app: AppHandle, state: AppStateRef) -> Result<(), String> {
    if !state.installs.lock().unwrap().is_empty() {
        return Err("Another install is in progress. Wait for it to finish first.".into());
    }
    let state = state.inner().clone();
    let id = state.catalog.diarization.id.clone();
    let name = state.catalog.diarization.name.clone();
    tauri::async_runtime::spawn(async move {
        let Some((cancel, _guard)) = crate::claim(&state.installs, &id) else { return };
        let emit = |phase: &str, received: u64, total: u64| {
            app.emit("install:progress", json!({ "model_id": id, "phase": phase, "received": received, "total": total })).ok();
        };
        let result = install(&state, &cancel, &emit).await;
        let payload = match &result {
            Ok(()) => json!({ "model_id": id, "name": name, "ok": true }),
            Err(err) if err == download::CANCELLED => json!({ "model_id": id, "name": name, "ok": false, "cancelled": true }),
            Err(err) => {
                state.log("model", &format!("Installing {name} failed: {err}"));
                json!({ "model_id": id, "name": name, "ok": false, "error": err })
            }
        };
        app.emit("install:finished", payload).ok();
    });
    Ok(())
}

#[tauri::command]
pub fn remove_speaker_model(state: AppStateRef) -> Result<(), String> {
    *slot().lock().unwrap() = None;
    let m = &state.catalog.diarization;
    let dir = state.paths.models.join(&m.id);
    if dir.starts_with(&state.paths.models) && dir != state.paths.models && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the model: {e}"))?;
    }
    db::log_action(&state.db.lock().unwrap(), "model", &format!("Removed {} and deleted its files", m.name));
    Ok(())
}

pub fn unload() {
    *slot().lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_finds_a_pure_tone() {
        let n = 512;
        let mut re: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * 32.0 * i as f32 / n as f32).cos()).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let mags: Vec<f32> = (0..n / 2).map(|i| (re[i] * re[i] + im[i] * im[i]).sqrt()).collect();
        let peak = mags.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, 32);
        assert!((mags[32] - 256.0).abs() < 0.5);
    }

    #[test]
    fn fbank_has_kaldi_frame_count_and_zero_mean() {
        let samples: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.07).sin() * 0.3).collect();
        let (frames, f) = fbank(&samples);
        assert_eq!(frames, 1 + (16_000 - 400) / 160);
        assert_eq!(f.len(), frames * 80);
        let mean0 = (0..frames).map(|t| f[t * 80 + 10]).sum::<f32>() / frames as f32;
        assert!(mean0.abs() < 1e-3);
    }

    #[test]
    fn a_tone_lights_up_the_right_band() {
        // 1 kHz sits around band 25 of 80 on this mel scale (before mean removal, so compare raw energies).
        let tone: Vec<f32> = (0..8_000).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 16_000.0).sin() * 0.5).collect();
        let mut mixed = tone.clone();
        mixed.extend(vec![0.0001f32; 8_000]);
        let (frames, f) = fbank(&mixed);
        // In tone frames the 1 kHz band is far above the mean; in quiet frames far below.
        let band = (0..80).max_by(|a, b| f[10 * 80 + a].total_cmp(&f[10 * 80 + b])).unwrap();
        assert!((20..32).contains(&band), "{band}");
        assert!(f[(frames - 5) * 80 + band] < 0.0);
    }

    #[test]
    fn grouping_separates_two_voices() {
        let a = normalize(vec![1.0, 0.1, 0.0]);
        let a2 = normalize(vec![0.9, 0.2, 0.05]);
        let b = normalize(vec![0.0, 0.1, 1.0]);
        let b2 = normalize(vec![0.1, 0.0, 0.95]);
        let mut live = Live::default();
        assert_eq!(live.assign(&a), 1);
        assert_eq!(live.assign(&b), 2);
        assert_eq!(live.assign(&a2), 1);
        assert_eq!(live.assign(&b2), 2);
        assert_eq!(regroup(&[b.clone(), a.clone(), b2, a2]), vec![1, 2, 1, 2], "numbered by who spoke first");
        assert!(regroup(&[]).is_empty());
    }
}

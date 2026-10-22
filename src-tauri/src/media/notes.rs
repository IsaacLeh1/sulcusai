// SPDX-License-Identifier: AGPL-3.0-only
//! The notes of a song's parts, for the song view and its MIDI file. A
//! melodic part (the voice, bass, other instruments) is read by Basic Pitch
//! (Spotify, Apache-2.0, on ONNX Runtime) with its own way of turning what
//! it hears into notes; drums are hits found by their onsets and sorted by
//! sound into kick, snare and hi-hat.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

/// A note: when it starts and ends (seconds), its MIDI pitch and loudness.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub start: f32,
    pub end: f32,
    pub pitch: u8,
    pub velocity: u8,
}

/// Basic Pitch's audio: 22.05 kHz mono, in windows of 2 s less one hop,
/// overlapping by 30 frames (of 256 samples).
pub const RATE: u32 = 22_050;
const HOP: usize = 256;
const WINDOW: usize = 2 * RATE as usize - HOP;
const OVERLAP_FRAMES: usize = 30;
const FRAMES_PER_WINDOW: usize = 172;
const FPS: f64 = (RATE as usize / HOP) as f64;
const KEYS: usize = 88;
/// MIDI number of the lowest key.
const LOWEST: u8 = 21;
/// Its defaults: onset and frame thresholds, the shortest note (127.7 ms in
/// frames) and how many quiet frames a note may run through.
const ONSET_THRESH: f32 = 0.5;
const FRAME_THRESH: f32 = 0.3;
const MIN_NOTE: usize = 11;
const ENERGY_TOL: usize = 11;

fn session() -> &'static Mutex<Option<(PathBuf, Session)>> {
    static S: OnceLock<Mutex<Option<(PathBuf, Session)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn e(err: impl std::fmt::Display) -> String {
    format!("Reading the notes failed: {err}")
}

pub fn unload() {
    *session().lock().unwrap() = None;
}

/// A frame-by-key grid, row-major.
struct Grid {
    rows: usize,
    data: Vec<f32>,
}

impl Grid {
    fn at(&self, t: usize, k: usize) -> f32 {
        self.data[t * KEYS + k]
    }
    fn set(&mut self, t: usize, k: usize, v: f32) {
        self.data[t * KEYS + k] = v;
    }
}

/// Basic Pitch's note and onset grids for 22.05 kHz mono audio, the windows'
/// overlaps trimmed and the frames cut to the audio's length.
fn grids(dll: &Path, model: &Path, audio: &[f32], threads: usize) -> Result<(Grid, Grid), String> {
    crate::natural::runtime_ready(dll)?;
    let mut guard = session().lock().unwrap();
    if guard.as_ref().map(|(p, _)| p.as_path()) != Some(model) {
        let s = Session::builder().map_err(e)?.with_intra_threads(threads.max(1)).map_err(e)?.commit_from_file(model).map_err(e)?;
        *guard = Some((model.to_path_buf(), s));
    }
    let sess = &mut guard.as_mut().unwrap().1;
    let overlap = OVERLAP_FRAMES * HOP;
    let hop = WINDOW - overlap;
    let mut padded = vec![0f32; overlap / 2];
    padded.extend_from_slice(audio);
    let keep = OVERLAP_FRAMES / 2;
    let (mut notes, mut onsets) = (Vec::new(), Vec::new());
    let mut start = 0;
    while start < padded.len() {
        let mut window = padded[start..(start + WINDOW).min(padded.len())].to_vec();
        window.resize(WINDOW, 0.0);
        let out = sess.run(ort::inputs! { "serving_default_input_2:0" => Tensor::from_array(([1usize, WINDOW, 1], window)).map_err(e)? }).map_err(e)?;
        // Its outputs: 0 the pitch contour, 1 the notes, 2 the onsets.
        for (name, into) in [("StatefulPartitionedCall:1", &mut notes), ("StatefulPartitionedCall:2", &mut onsets)] {
            let (_, v) = out[name].try_extract_tensor::<f32>().map_err(e)?;
            if v.len() != FRAMES_PER_WINDOW * KEYS {
                return Err(e("unexpected output size"));
            }
            into.extend_from_slice(&v[keep * KEYS..(FRAMES_PER_WINDOW - keep) * KEYS]);
        }
        start += hop;
    }
    let rows = ((audio.len() as f64 * FPS / RATE as f64).floor() as usize).min(notes.len() / KEYS);
    notes.truncate(rows * KEYS);
    onsets.truncate(rows * KEYS);
    Ok((Grid { rows, data: notes }, Grid { rows, data: onsets }))
}

/// Onsets where a note's energy rises (two frames' rise, scaled to the
/// onsets' peak), as Basic Pitch infers them, kept where stronger.
fn inferred_onsets(frames: &Grid, onsets: &Grid) -> Grid {
    let mut diff = vec![0f32; frames.data.len()];
    for t in 2..frames.rows {
        for k in 0..KEYS {
            let d = (frames.at(t, k) - frames.at(t - 1, k)).min(frames.at(t, k) - frames.at(t - 2, k));
            diff[t * KEYS + k] = d.max(0.0);
        }
    }
    let peak_on = onsets.data.iter().fold(0f32, |m, v| m.max(*v));
    let peak_diff = diff.iter().fold(0f32, |m, v| m.max(*v));
    let scale = if peak_diff > 0.0 { peak_on / peak_diff } else { 0.0 };
    Grid { rows: onsets.rows, data: onsets.data.iter().zip(&diff).map(|(o, d)| o.max(d * scale)).collect() }
}

/// Notes from the grids (Basic Pitch's `output_to_notes_polyphonic`, with
/// its "melodia trick" for notes without a clear onset). Frames, MIDI pitch,
/// mean energy.
fn grid_notes(frames: &Grid, onsets: &Grid) -> Vec<(usize, usize, u8, f32)> {
    let n = frames.rows;
    if n < 3 {
        return Vec::new();
    }
    let onsets = inferred_onsets(frames, onsets);
    let mut remaining = Grid { rows: n, data: frames.data.clone() };
    let mut starts = Vec::new();
    for t in 1..n - 1 {
        for k in 0..KEYS {
            let v = onsets.at(t, k);
            if v >= ONSET_THRESH && v > onsets.at(t - 1, k) && v > onsets.at(t + 1, k) {
                starts.push((t, k));
            }
        }
    }
    let clear = |g: &mut Grid, t: usize, k: usize| {
        g.set(t, k, 0.0);
        if k + 1 < KEYS {
            g.set(t, k + 1, 0.0);
        }
        if k > 0 {
            g.set(t, k - 1, 0.0);
        }
    };
    let mean = |a: usize, b: usize, k: usize| (a..b).map(|t| frames.at(t, k)).sum::<f32>() / (b - a).max(1) as f32;
    let mut out = Vec::new();
    // Latest first, as Basic Pitch does.
    for &(t0, k) in starts.iter().rev() {
        if t0 >= n - 1 {
            continue;
        }
        let (mut i, mut quiet) = (t0 + 1, 0);
        while i < n - 1 && quiet < ENERGY_TOL {
            quiet = if remaining.at(i, k) < FRAME_THRESH { quiet + 1 } else { 0 };
            i += 1;
        }
        i -= quiet;
        if i - t0 <= MIN_NOTE {
            continue;
        }
        for t in t0..i {
            clear(&mut remaining, t, k);
        }
        out.push((t0, i, LOWEST + k as u8, mean(t0, i, k)));
    }
    // The melodia trick: what's left that's loud enough, from the loudest,
    // followed both ways.
    loop {
        let (idx, peak) = remaining.data.iter().enumerate().fold((0, 0f32), |(bi, bv), (i, v)| if *v > bv { (i, *v) } else { (bi, bv) });
        if peak <= FRAME_THRESH {
            break;
        }
        let (mid, k) = (idx / KEYS, idx % KEYS);
        remaining.set(mid, k, 0.0);
        let (mut i, mut quiet) = (mid + 1, 0);
        while i < n - 1 && quiet < ENERGY_TOL {
            quiet = if remaining.at(i, k) < FRAME_THRESH { quiet + 1 } else { 0 };
            clear(&mut remaining, i, k);
            i += 1;
        }
        let end = i - 1 - quiet;
        let (mut i, mut quiet) = (mid as isize - 1, 0);
        while i > 0 && quiet < ENERGY_TOL {
            quiet = if remaining.at(i as usize, k) < FRAME_THRESH { quiet + 1 } else { 0 };
            clear(&mut remaining, i as usize, k);
            i -= 1;
        }
        let start = (i + 1 + quiet as isize).max(0) as usize;
        if end <= start || end - start <= MIN_NOTE {
            continue;
        }
        out.push((start, end, LOWEST + k as u8, mean(start, end, k)));
    }
    out
}

/// A frame's time: each window's frames run a little behind (Basic Pitch's
/// `model_frames_to_time`).
fn frame_time(f: usize) -> f32 {
    let window_offset = (HOP as f64 / RATE as f64) * (FRAMES_PER_WINDOW as f64 - WINDOW as f64 / HOP as f64) + 0.0018;
    (f as f64 / FPS - window_offset * (f / FRAMES_PER_WINDOW) as f64).max(0.0) as f32
}

/// The notes of a melodic part, from 22.05 kHz mono audio, in time order.
pub fn melodic(dll: &Path, model: &Path, audio: &[f32], threads: usize) -> Result<Vec<Note>, String> {
    let (frames, onsets) = grids(dll, model, audio, threads)?;
    let mut notes: Vec<Note> = grid_notes(&frames, &onsets)
        .into_iter()
        .map(|(a, b, pitch, amp)| Note { start: frame_time(a), end: frame_time(b), pitch, velocity: (amp * 127.0).round().clamp(1.0, 127.0) as u8 })
        .collect();
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    Ok(notes)
}

/// General MIDI's drums: kick, snare, closed hi-hat.
pub const KICK: u8 = 36;
pub const SNARE: u8 = 38;
pub const HAT: u8 = 42;

/// Drum hits in a drum part (mono): where the sound jumps (spectral flux
/// over 23 ms frames), and which drum by where its energy is: low (kick),
/// middle (snare) or high (hi-hat), where that band's sound leads. At most
/// one hit of a drum every 60 ms.
pub fn drum_hits(audio: &[f32], rate: u32) -> Vec<Note> {
    use realfft::RealFftPlanner;
    let n = 1024usize;
    let hop = 512usize;
    if audio.len() < n * 2 {
        return Vec::new();
    }
    let frames = (audio.len() - n) / hop + 1;
    let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
    let bin = |hz: f32| ((hz * n as f32 / rate as f32) as usize).min(n / 2);
    let bands = [(bin(30.0), bin(150.0)), (bin(150.0), bin(2500.0)), (bin(5000.0), bin(16_000.0))];
    let mut energy = vec![[0f32; 3]; frames];
    let mut buf = vec![0f32; n];
    let mut spec = fft.make_output_vec();
    for (f, en) in energy.iter_mut().enumerate() {
        for k in 0..n {
            buf[k] = audio[f * hop + k] * win[k];
        }
        fft.process(&mut buf, &mut spec).ok();
        for (b, (lo, hi)) in bands.iter().enumerate() {
            en[b] = spec[*lo..*hi].iter().map(|c| c.norm()).sum::<f32>();
        }
    }
    let mut hits = Vec::new();
    for (b, pitch) in [KICK, SNARE, HAT].into_iter().enumerate() {
        // The rise of the band's log energy, against its local average.
        let log: Vec<f32> = energy.iter().map(|e| (1.0 + e[b]).ln()).collect();
        // A hit at the very start rises from silence.
        let flux: Vec<f32> = (0..frames).map(|f| if f == 0 { log[0] } else { (log[f] - log[f - 1]).max(0.0) }).collect();
        let peak = flux.iter().fold(0f32, |m, v| m.max(*v));
        if peak <= 0.0 {
            continue;
        }
        let gap = ((0.06 * rate as f32) / hop as f32).ceil() as usize;
        let mut last: Option<usize> = None;
        for f in 0..frames.saturating_sub(1) {
            let around = &flux[f.saturating_sub(8)..(f + 8).min(frames)];
            let avg = around.iter().sum::<f32>() / around.len() as f32;
            let strong = flux[f] > 0.25 * peak && flux[f] > 1.8 * avg && (f == 0 || flux[f] >= flux[f - 1]) && flux[f] >= flux[f + 1];
            // Its own sound: a kick is mostly low, a snare mostly middle, a
            // hi-hat bright (another drum's spill into the band isn't a hit).
            let e = energy[f];
            let its_own = match b {
                0 => e[0] >= e[1],
                1 => e[1] >= e[0],
                _ => e[2] >= 0.5 * e[1],
            };
            let strong = strong && its_own;
            if strong && last.is_none_or(|l| f - l >= gap) {
                last = Some(f);
                let t = (f * hop) as f32 / rate as f32;
                hits.push(Note { start: t, end: t + 0.1, pitch, velocity: (40.0 + 87.0 * (flux[f] / peak)).min(127.0) as u8 });
            }
        }
    }
    hits.sort_by(|a, b| a.start.total_cmp(&b.start));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two held notes written into the grids as Basic Pitch would hear them:
    /// both come out, with the right pitch, start and length; a short blip
    /// doesn't.
    #[test]
    fn notes_come_out_of_the_grids() {
        let rows = 200;
        let mut frames = Grid { rows, data: vec![0.0; rows * KEYS] };
        let mut onsets = Grid { rows, data: vec![0.0; rows * KEYS] };
        let mut note = |k: usize, a: usize, b: usize| {
            for t in a..b {
                frames.set(t, k, 0.8);
            }
            onsets.set(a, k, 0.9);
        };
        note(39, 10, 60); // C4 (MIDI 60), 50 frames
        note(43, 80, 150); // E4 (MIDI 64)
        note(50, 170, 175); // too short
        let mut found = grid_notes(&frames, &onsets);
        found.sort_by_key(|n| (n.0, n.2));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((found[0].0, found[0].1, found[0].2), (10, 60, 60));
        assert_eq!((found[1].0, found[1].1, found[1].2), (80, 150, 64));
        assert!((found[0].3 - 0.8).abs() < 1e-3);
        assert!((frame_time(86) - 1.0).abs() < 0.01 && frame_time(172) < 2.0, "a window's frames run a little behind");
    }

    /// A beat of kicks on the beats and hi-hats between: each is found as its drum.
    #[test]
    fn drum_hits_are_found_and_sorted() {
        let rate = 44_100u32;
        let mut x = vec![0f32; rate as usize * 2];
        let mut noise = 12345u64;
        let mut rnd = || {
            noise ^= noise << 13;
            noise ^= noise >> 7;
            noise ^= noise << 17;
            (noise >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
        };
        for beat in 0..4 {
            let at = (beat as f32 * 0.5 * rate as f32) as usize;
            for i in 0..4000 {
                let t = i as f32 / rate as f32;
                x[at + i] += (2.0 * std::f32::consts::PI * 60.0 * t).sin() * (-t * 30.0).exp() * 0.8;
            }
            let hat = at + rate as usize / 4;
            let mut prev = 0.0;
            for i in 0..1500 {
                // A bright click: noise differenced (high frequencies only).
                let v = rnd();
                x[hat + i] += (v - prev) * 0.3 * (-(i as f32) / 300.0).exp();
                prev = v;
            }
        }
        let hits = drum_hits(&x, rate);
        let kicks: Vec<f32> = hits.iter().filter(|h| h.pitch == KICK).map(|h| h.start).collect();
        let hats = hits.iter().filter(|h| h.pitch == HAT).count();
        assert_eq!(kicks.len(), 4, "{hits:?}");
        assert!(kicks.iter().enumerate().all(|(i, t)| (t - i as f32 * 0.5).abs() < 0.03), "{kicks:?}");
        assert!(hats >= 3, "{hits:?}");
    }
}

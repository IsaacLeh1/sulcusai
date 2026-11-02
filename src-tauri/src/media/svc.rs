// SPDX-License-Identifier: AGPL-3.0-only
//! Singing in the person's own voice: Seed-VC's singing voice conversion
//! (v1, 44.1 kHz, following the sung pitch; GPL-3.0, github.com/Plachtaa/
//! seed-vc), exported to ONNX in eight pieces (`scripts/voice/
//! export_svc.py`) and run here the way its `inference.py` runs it:
//!
//! - what is sung: Whisper small's encoder on the 16 kHz voice
//!   (`whisper_mel`, `whisper`), 50 frames a second, 30 s at a time;
//! - its pitch: RMVPE (`rmvpe`), every 10 ms;
//! - the person's voice: CAMPPlus (`campplus`) on Kaldi filterbanks of their
//!   recording, and its mel spectrogram (`mel44`) as the prompt;
//! - both put on the 44.1 kHz mel frames (`regulator`);
//! - the new mel spectrogram by flow matching (`dit`, with guidance), in
//!   windows of 30 s less the prompt that overlap by 16 frames;
//! - the sound, by BigVGAN v2 44 kHz (`bigvgan`).
//!
//! The ONNX pieces matched Seed-VC's own run to about 1e-4 (relative), and
//! the whole chain from the same noise to 2e-5 of its output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ort::session::Session;
use ort::value::Tensor;

use super::stems;

/// The rate of everything sung and made here.
pub const RATE: u32 = 44_100;
const HOP: usize = 512;
const MELS: usize = 128;
const WIDTH: usize = 768;
/// Frames in one window: 30 s (`sr // hop_length * 30`).
const WINDOW: usize = (44_100 / HOP) * 30;
/// Frames two windows share, cross-faded.
const OVERLAP: usize = 16;
/// The longest recording of the person used as the prompt.
pub const MAX_PROMPT_S: f32 = 12.0;
/// Pitch below this (Hz) is no pitch.
const VOICED: f32 = 1.0;
/// The pieces, by file name.
pub const PIECES: [&str; 8] = ["whisper_mel.onnx", "whisper.onnx", "rmvpe.onnx", "campplus.onnx", "mel44.onnx", "regulator.onnx", "dit.onnx", "bigvgan.onnx"];

fn e(err: impl std::fmt::Display) -> String {
    format!("Singing in your voice failed: {err}")
}

/// How a conversion runs.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Flow-matching steps: 30 is Seed-VC's own; fewer is faster and rougher.
    pub steps: usize,
    /// Guidance toward the person's voice (Seed-VC's default 0.7).
    pub cfg: f32,
    /// The tune moved by this many semitones (an octave is 12).
    pub semitones: i32,
    pub seed: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { steps: 30, cfg: 0.7, semitones: 0, seed: 7 }
    }
}

/// The person's voice, made ready once for a song: the prompt's mel frames,
/// its condition, the voice print and the voice's middle pitch.
pub struct Voice {
    mel: Vec<f32>,
    frames: usize,
    condition: Vec<f32>,
    style: Vec<f32>,
    /// The middle of the voiced pitch (Hz), or 0.
    pub pitch: f32,
}

/// The open pieces.
pub struct Converter {
    dir: PathBuf,
    threads: usize,
    card: Option<crate::natural::Card>,
    /// The pieces opened so far, and whether each is on the graphics card.
    open: HashMap<&'static str, (Session, bool)>,
    /// Every piece run so far ran on the graphics card.
    pub on_card: bool,
    /// The last audio's pitch, by a fingerprint of the whole sound: songs
    /// of the same length that start in silence are told apart.
    pitch_of: Option<(u64, Vec<f32>)>,
}

/// The most each piece may take on the graphics card (past it, that piece
/// goes on on the processor): the transformer's attention over a 30 s
/// window, for both guidance halves, is ~1.3 GB besides its weights.
fn arena_of(piece: &str) -> usize {
    match piece {
        "dit.onnx" => 4 << 30,
        "bigvgan.onnx" => 3 << 30,
        "rmvpe.onnx" | "whisper.onnx" => 2 << 30,
        _ => 1 << 30,
    }
}

impl Converter {
    pub fn new(dll: &Path, dir: &Path, threads: usize, card: Option<crate::natural::Card>) -> Result<Converter, String> {
        crate::natural::runtime_ready(dll)?;
        for p in PIECES {
            if !dir.join(p).is_file() {
                return Err(e(format!("{p} is missing")));
            }
        }
        Ok(Converter { dir: dir.to_path_buf(), threads, card, open: HashMap::new(), on_card: card.is_some(), pitch_of: None })
    }

    /// Closes pieces that aren't needed any more: each keeps the most it took
    /// on the card until it's closed, and together they filled a 12 GB card,
    /// which then swapped and crawled (a 3-minute song took 20 minutes).
    fn close(&mut self, pieces: &[&str]) {
        self.open.retain(|p, _| !pieces.contains(p));
    }

    /// Runs a piece on float inputs (name, shape, values): its first output.
    /// Should the card fail at it (out of room), it goes on on the processor.
    pub(crate) fn run(&mut self, piece: &'static str, inputs: Vec<(&'static str, Vec<i64>, Vec<f32>)>) -> Result<Vec<f32>, String> {
        if !self.open.contains_key(piece) {
            let (s, on) = crate::natural::session_within(&self.dir.join(piece), self.threads, self.card, arena_of(piece))?;
            self.open.insert(piece, (s, on));
        }
        let once = |s: &mut Session, inputs: Vec<(&'static str, Vec<i64>, Vec<f32>)>| -> Result<Vec<f32>, String> {
            let feeds: Vec<(&str, ort::value::DynTensor)> =
                inputs.into_iter().map(|(n, shape, v)| Ok((n, Tensor::from_array((shape, v)).map_err(e)?.upcast()))).collect::<Result<_, String>>()?;
            s.run(feeds).and_then(|out| Ok(out[0].try_extract_tensor::<f32>()?.1.to_vec())).map_err(e)
        };
        let (s, on) = self.open.get_mut(piece).unwrap();
        if !*on {
            self.on_card = false;
            return once(s, inputs);
        }
        match once(s, inputs.clone()) {
            Ok(v) => Ok(v),
            Err(err) => {
                if !crate::cardfirst::go_on_on_processor("Singing it in the voice", &format!("the graphics card failed partway ({})", err.lines().next().unwrap_or(""))) {
                    return Err(crate::download::CANCELLED.into());
                }
                crate::applog::line("media", &format!("The graphics card couldn't run {piece} ({err}); going on on the processor"));
                let (s, _) = crate::natural::session_within(&self.dir.join(piece), self.threads, None, arena_of(piece))?;
                self.open.insert(piece, (s, false));
                self.on_card = false;
                once(&mut self.open.get_mut(piece).unwrap().0, inputs)
            }
        }
    }

    /// Whisper small's encoder output for 16 kHz audio (50 frames a second,
    /// `len / 320 + 1` of them), 30 s at a time with 5 s shared between
    /// pieces (`inference.py`'s way for long songs).
    fn content(&mut self, w16: &[f32]) -> Result<(usize, Vec<f32>), String> {
        const PIECE: usize = 16_000 * 30;
        const SHARED: usize = 16_000 * 5;
        let once = |s: &mut Self, chunk: &[f32]| -> Result<(usize, Vec<f32>), String> {
            let mut padded = vec![0f32; PIECE];
            let n = chunk.len().min(PIECE);
            padded[..n].copy_from_slice(&chunk[..n]);
            let mel = s.run("whisper_mel.onnx", vec![("wave", vec![1, PIECE as i64], padded)])?;
            let out = s.run("whisper.onnx", vec![("mel", vec![1, 80, 3000], mel)])?;
            let frames = (chunk.len() / 320 + 1).min(1500);
            Ok((frames, out[..frames * WIDTH].to_vec()))
        };
        if w16.len() <= PIECE {
            return once(self, w16);
        }
        let mut all = Vec::new();
        let mut frames = 0;
        let mut traversed = 0usize;
        let mut buffer: Option<Vec<f32>> = None;
        while traversed < w16.len() {
            let chunk: Vec<f32> = match &buffer {
                None => w16[..PIECE.min(w16.len())].to_vec(),
                Some(b) => {
                    let mut c = b.clone();
                    c.extend_from_slice(&w16[traversed..(traversed + PIECE - SHARED).min(w16.len())]);
                    c
                }
            };
            let (n, s) = once(self, &chunk)?;
            let skip = if buffer.is_none() { 0 } else { (50 * 5).min(n) };
            all.extend_from_slice(&s[skip * WIDTH..n * WIDTH]);
            frames += n - skip;
            buffer = Some(chunk[chunk.len() - SHARED.min(chunk.len())..].to_vec());
            traversed += if traversed == 0 { PIECE } else { chunk.len() - SHARED };
        }
        Ok((frames, all))
    }

    /// RMVPE's pitch every 10 ms of 16 kHz audio (0 where there's none), 30 s
    /// at a time with a second either side for context: a whole song at once
    /// took gigabytes of the card. The last song's is kept, as the song's
    /// middle pitch is asked for before it's converted.
    fn pitch(&mut self, w16: &[f32]) -> Result<Vec<f32>, String> {
        let print = fingerprint(w16);
        if let Some((known, f0)) = &self.pitch_of {
            if *known == print {
                return Ok(f0.clone());
            }
        }
        const HOP: usize = 160;
        const PIECE: usize = 16_000 * 30;
        const EDGE: usize = 16_000;
        let whole = |s: &mut Self, w: &[f32]| -> Result<Vec<f32>, String> {
            let salience = s.run("rmvpe.onnx", vec![("wave", vec![1, w.len() as i64], w.to_vec())])?;
            Ok(decode_pitch(&salience, 0.03))
        };
        let f0 = if w16.len() <= PIECE + 2 * EDGE {
            whole(self, w16)?
        } else {
            let frames = w16.len() / HOP + 1;
            let mut f0 = Vec::with_capacity(frames);
            let mut start = 0;
            while start < w16.len() {
                let (a, b) = (start.saturating_sub(EDGE), (start + PIECE + EDGE).min(w16.len()));
                let p = whole(self, &w16[a..b])?;
                let skip = (start - a) / HOP;
                let take = ((start + PIECE).min(w16.len()) - start) / HOP;
                f0.extend_from_slice(&p[skip.min(p.len())..(skip + take).min(p.len())]);
                start += PIECE;
            }
            f0.resize(frames, 0.0);
            f0
        };
        self.pitch_of = Some((print, f0.clone()));
        Ok(f0)
    }

    /// The person's voice from their recording (44.1 kHz, `MAX_PROMPT_S` at most).
    pub fn voice(&mut self, recording: &[f32]) -> Result<Voice, String> {
        let rec = &recording[..recording.len().min((MAX_PROMPT_S * RATE as f32) as usize)];
        if rec.len() < RATE as usize {
            return Err(e("the recording of your voice is too short"));
        }
        let w16 = stems::resample(rec, RATE, 16_000);
        let (n, content) = self.content(&w16)?;
        let f0 = self.pitch(&w16)?;
        let (fb_frames, fb) = crate::diarize::fbank_with(&w16, true);
        let style = self.run("campplus.onnx", vec![("fbank", vec![1, fb_frames as i64, 80], fb)])?;
        let mel = self.run("mel44.onnx", vec![("wave", vec![1, rec.len() as i64], rec.to_vec())])?;
        let frames = mel.len() / MELS;
        let condition = self.regulate(&content, n, &f0, frames)?;
        Ok(Voice { mel, frames, condition, style, pitch: middle_pitch(&f0) })
    }

    /// Content (`n` frames of `WIDTH`) and pitch put on `frames` mel frames.
    fn regulate(&mut self, content: &[f32], n: usize, f0: &[f32], frames: usize) -> Result<Vec<f32>, String> {
        let idx = nearest(n, frames);
        let mut c = Vec::with_capacity(frames * WIDTH);
        for &i in &idx {
            c.extend_from_slice(&content[i * WIDTH..(i + 1) * WIDTH]);
        }
        let p: Vec<f32> = nearest(f0.len(), frames).into_iter().map(|i| f0[i]).collect();
        self.run("regulator.onnx", vec![("content", vec![1, frames as i64, WIDTH as i64], c), ("f0", vec![1, frames as i64], p)])
    }

    /// The middle pitch of sung audio (44.1 kHz), for fitting a song to the
    /// person's range.
    pub fn middle_pitch_of(&mut self, sung: &[f32]) -> Result<f32, String> {
        let w16 = stems::resample(sung, RATE, 16_000);
        Ok(middle_pitch(&self.pitch(&w16)?))
    }

    /// The pitch of sung audio (44.1 kHz) every 10 ms, in Hz (0 where
    /// there's none).
    pub fn pitch_track(&mut self, sung: &[f32]) -> Result<Vec<f32>, String> {
        let w16 = stems::resample(sung, RATE, 16_000);
        self.pitch(&w16)
    }

    /// A sung part (44.1 kHz mono) in the person's voice; `progress` gets
    /// 0..1.
    pub fn convert(&mut self, voice: &Voice, sung: &[f32], s: Settings, cancel: &AtomicBool, progress: &dyn Fn(f64)) -> Result<Vec<f32>, String> {
        let stopped = || if cancel.load(Ordering::Relaxed) { Err(crate::download::CANCELLED.to_string()) } else { Ok(()) };
        let w16 = stems::resample(sung, RATE, 16_000);
        let (n, content) = self.content(&w16)?;
        stopped()?;
        let mut f0 = self.pitch(&w16)?;
        stopped()?;
        if s.semitones != 0 {
            let k = 2f32.powf(s.semitones as f32 / 12.0);
            f0.iter_mut().filter(|v| **v > VOICED).for_each(|v| *v *= k);
        }
        let mel = self.run("mel44.onnx", vec![("wave", vec![1, sung.len() as i64], sung.to_vec())])?;
        let frames = mel.len() / MELS;
        drop(mel);
        let cond = self.regulate(&content, n, &f0, frames)?;
        // Only the transformer and the vocoder are needed from here.
        self.close(&["whisper_mel.onnx", "whisper.onnx", "rmvpe.onnx", "campplus.onnx", "mel44.onnx", "regulator.onnx"]);
        progress(0.1);
        // Only where there's singing: the quiet stretches (an intro, a break
        // for the instruments) stay quiet without being worked on.
        let sung_at = singing(sung, frames);
        let work: usize = sung_at.iter().map(|(a, b)| b - a).sum::<usize>().max(1);
        let room = WINDOW.saturating_sub(voice.frames).max(OVERLAP * 4);
        let mut rng = Noise::new(s.seed);
        let mut out = vec![0f32; frames.max(1) * HOP];
        let overlap = OVERLAP * HOP;
        let mut before = 0usize;
        for &(from, to) in &sung_at {
            let mut piece_out: Vec<f32> = Vec::with_capacity((to - from) * HOP);
            let mut previous: Option<Vec<f32>> = None;
            let mut done = from;
            while done < to {
                if cancel.load(Ordering::Relaxed) {
                    return Err(crate::download::CANCELLED.into());
                }
                let end = (done + room).min(to);
                let last = done + room >= to;
                let noise: Vec<f32> = (0..MELS * (voice.frames + end - done)).map(|_| rng.normal()).collect();
                let at = before + done - from;
                let mel = self.sample(voice, &cond[done * WIDTH..end * WIDTH], end - done, s, noise, cancel, &|f| progress(0.1 + 0.85 * (at as f64 + f * (end - done) as f64) / work as f64))?;
                let wave = self.run("bigvgan.onnx", vec![("mel", vec![1, MELS as i64, (end - done) as i64], mel)])?;
                match previous.take() {
                    None if last => piece_out.extend_from_slice(&wave),
                    None => {
                        piece_out.extend_from_slice(&wave[..wave.len() - overlap]);
                        previous = Some(wave[wave.len() - overlap..].to_vec());
                    }
                    Some(prev) => {
                        let keep = if last { wave.len() } else { wave.len() - overlap };
                        let mut piece = wave[..keep].to_vec();
                        crossfade(&prev, &mut piece);
                        piece_out.extend_from_slice(&piece);
                        if !last {
                            previous = Some(wave[wave.len() - overlap..].to_vec());
                        }
                    }
                }
                if last {
                    break;
                }
                done += (end - done) - OVERLAP;
            }
            fade_ends(&mut piece_out);
            let start = from * HOP;
            let n = piece_out.len().min(out.len().saturating_sub(start));
            out[start..start + n].copy_from_slice(&piece_out[..n]);
            before += to - from;
        }
        progress(1.0);
        out.resize(sung.len(), 0.0);
        Ok(out)
    }

    /// The end of a short stretch of live speech (44.1 kHz) in the voice:
    /// `window` is what was just said with a little before it (so the end
    /// has its context), and the last `keep` samples come back. Nothing is
    /// closed between calls, as a song's conversion does.
    pub fn convert_live(&mut self, voice: &Voice, window: &[f32], keep: usize, s: Settings, noise: &mut Noise) -> Result<Vec<f32>, String> {
        let w16 = stems::resample(window, RATE, 16_000);
        let (n, content) = self.content(&w16)?;
        let mut f0 = self.pitch(&w16)?;
        if s.semitones != 0 {
            let k = 2f32.powf(s.semitones as f32 / 12.0);
            f0.iter_mut().filter(|v| **v > VOICED).for_each(|v| *v *= k);
        }
        let frames = self.run("mel44.onnx", vec![("wave", vec![1, window.len() as i64], window.to_vec())])?.len() / MELS;
        let cond = self.regulate(&content, n, &f0, frames)?;
        let x: Vec<f32> = (0..MELS * (voice.frames + frames)).map(|_| noise.normal()).collect();
        let mel = self.sample(voice, &cond, frames, s, x, &AtomicBool::new(false), &|_| {})?;
        // Only the end is heard: the vocoder gets it with a little before it,
        // enough for its own context (it took most of the time on a whole window).
        let tail = (keep.div_ceil(HOP) + OVERLAP).min(frames);
        let mut end = Vec::with_capacity(MELS * tail);
        for m in 0..MELS {
            end.extend_from_slice(&mel[m * frames + frames - tail..(m + 1) * frames]);
        }
        let wave = self.run("bigvgan.onnx", vec![("mel", vec![1, MELS as i64, tail as i64], end)])?;
        Ok(wave[wave.len().saturating_sub(keep)..].to_vec())
    }

    /// One window's mel spectrogram (`frames` of the song), the person's
    /// prompt before it, by Euler steps of the flow with guidance from
    /// `noise` (MELS x (prompt + frames)). Stopping is checked at every step.
    #[allow(clippy::too_many_arguments)]
    fn sample(&mut self, voice: &Voice, cond: &[f32], frames: usize, s: Settings, noise: Vec<f32>, cancel: &AtomicBool, progress: &dyn Fn(f64)) -> Result<Vec<f32>, String> {
        let p = voice.frames;
        let total = p + frames;
        let mut condition = Vec::with_capacity(total * WIDTH);
        condition.extend_from_slice(&voice.condition);
        condition.extend_from_slice(cond);
        // (MELS, total), row by row; the prompt's columns start at zero.
        let mut x = noise;
        let mut prompt = vec![0f32; MELS * total];
        for m in 0..MELS {
            x[m * total..m * total + p].iter_mut().for_each(|v| *v = 0.0);
            prompt[m * total..m * total + p].copy_from_slice(&voice.mel[m * p..(m + 1) * p]);
        }
        let steps = s.steps.max(1);
        let dt = 1.0 / steps as f32;
        for k in 0..steps {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::download::CANCELLED.into());
            }
            let t = k as f32 * dt;
            let v = self.run(
                "dit.onnx",
                vec![
                    ("x", vec![1, MELS as i64, total as i64], x.clone()),
                    ("prompt", vec![1, MELS as i64, total as i64], prompt.clone()),
                    ("condition", vec![1, total as i64, WIDTH as i64], condition.clone()),
                    ("style", vec![1, 192], voice.style.clone()),
                    ("t", vec![1], vec![t]),
                    ("cfg", vec![1], vec![s.cfg]),
                ],
            )?;
            for (a, b) in x.iter_mut().zip(&v) {
                *a += dt * b;
            }
            for m in 0..MELS {
                x[m * total..m * total + p].iter_mut().for_each(|v| *v = 0.0);
            }
            progress((k + 1) as f64 / steps as f64);
        }
        let mut mel = Vec::with_capacity(MELS * frames);
        for m in 0..MELS {
            mel.extend_from_slice(&x[m * total + p..(m + 1) * total]);
        }
        Ok(mel)
    }
}

/// The stretches of a sung part (in mel frames, `frames` of them) with
/// singing in them: frames louder than 45 dB under the loudest, with gaps
/// under 2 s joined and half a second kept either side, so a breath or a
/// held note's tail isn't cut. A part that's sung throughout is one stretch.
fn singing(sung: &[f32], frames: usize) -> Vec<(usize, usize)> {
    let loud: Vec<f32> = (0..frames)
        .map(|f| {
            let a = &sung[(f * HOP).min(sung.len())..((f + 1) * HOP).min(sung.len())];
            (a.iter().map(|v| v * v).sum::<f32>() / a.len().max(1) as f32).sqrt()
        })
        .collect();
    let peak = loud.iter().copied().fold(0f32, f32::max);
    if peak <= 0.0 {
        return Vec::new();
    }
    let floor = peak * 10f32.powf(-45.0 / 20.0);
    let (gap, pad) = (2 * RATE as usize / HOP, RATE as usize / HOP / 2);
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (f, _) in loud.iter().enumerate().filter(|(_, l)| **l > floor) {
        let (a, b) = (f.saturating_sub(pad), (f + 1 + pad).min(frames));
        match out.last_mut() {
            Some(last) if a <= last.1 + gap => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// A short fade in and out (10 ms), so a stretch starts and ends without a click.
fn fade_ends(wave: &mut [f32]) {
    let n = (RATE as usize / 100).min(wave.len() / 2);
    for i in 0..n {
        let g = i as f32 / n as f32;
        wave[i] *= g;
        let j = wave.len() - 1 - i;
        wave[j] *= g;
    }
}

/// `F.interpolate(mode="nearest")`'s source frame for each of `size` frames.
fn nearest(n: usize, size: usize) -> Vec<usize> {
    let scale = n as f32 / size.max(1) as f32;
    (0..size).map(|i| ((i as f32 * scale).floor() as usize).min(n.saturating_sub(1))).collect()
}

/// RMVPE's pitch from its salience (frames x 360): the weighted cents around
/// each frame's peak, none where the peak is under `threshold`.
fn decode_pitch(salience: &[f32], threshold: f32) -> Vec<f32> {
    const BINS: usize = 360;
    let cents = |i: isize| if (0..BINS as isize).contains(&i) { 20.0 * i as f32 + 1997.379_4 } else { 0.0 };
    salience
        .chunks(BINS)
        .map(|row| {
            let (peak, max) = row.iter().enumerate().fold((0, f32::MIN), |(bi, bv), (i, &v)| if v > bv { (i, v) } else { (bi, bv) });
            if max <= threshold {
                return 0.0;
            }
            let (mut num, mut den) = (0f32, 0f32);
            for j in peak as isize - 4..=peak as isize + 4 {
                let w = if (0..BINS as isize).contains(&j) { row[j as usize] } else { 0.0 };
                num += w * cents(j);
                den += w;
            }
            let c = num / den;
            let f = 10.0 * 2f32.powf(c / 1200.0);
            if (f - 10.0).abs() < 1e-6 { 0.0 } else { f }
        })
        .collect()
}

/// FNV-1a over the samples' bits and the length.
fn fingerprint(w: &[f32]) -> u64 {
    w.iter().fold(0xcbf2_9ce4_8422_2325 ^ w.len() as u64, |h, s| (h ^ s.to_bits() as u64).wrapping_mul(0x0100_0000_01b3))
}

fn middle_pitch(f0: &[f32]) -> f32 {
    let mut v: Vec<f32> = f0.iter().copied().filter(|x| *x > VOICED).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// The notes a pitch track spans (Hz), low and high, leaving out the
/// lowest and highest 5% of what's voiced: a crack, a breath or a wrong
/// reading of the pitch shouldn't stretch a singer's range.
pub fn range_of(f0: &[f32]) -> Option<(f32, f32)> {
    let mut v: Vec<f32> = f0.iter().copied().filter(|x| *x > VOICED).collect();
    if v.len() < 20 {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some((v[v.len() / 20], v[v.len() - 1 - v.len() / 20]))
}

/// A pitch as a note name ("A3", "C#4").
pub fn note_name(hz: f32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    if hz <= VOICED {
        return "-".into();
    }
    let midi = (69.0 + 12.0 * (hz / 440.0).log2()).round() as i32;
    format!("{}{}", NAMES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

/// Where a song goes for a singer: semitones for its voice, and for the
/// band (all but the drums) when the song's key has to change too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub voice: i32,
    pub band: i32,
}

/// What moving the band costs, against singing outside the person's range
/// (squared semitones, over the whole tune): something to move it at all,
/// so a song that nearly fits isn't put through the phase vocoder for a
/// semitone; then little a semitone, as a song moved five semitones kept
/// its words and likeness as well as one left alone, but of two keys as
/// good, the nearer.
const BAND_MOVED: f32 = 0.1;
const BAND_MOVE: f32 = 0.05;
/// What a tune sitting off the middle of the person's range costs, a
/// semitone at a time past the first two: of the keys that fit, the one
/// that uses their range rather than hugging its bottom or top, with no
/// move made for a semitone or two.
const OFF_MIDDLE: f32 = 0.15;
const MIDDLE_EASE: f32 = 2.0;

/// The key a song is sung in by the person: whole octaves for the voice and
/// up to five semitones up or down for the whole song, chosen so the tune
/// (its pitch track) stays inside the person's sung range, as a singer
/// asks for a song in their key. Notes outside the range count by the
/// square of how far out they are (three semitones over is far worse than
/// one), over the whole tune, so a chorus's few high notes weigh less than
/// every line of the verses. Moving by octaves alone left a man's song
/// four semitones over a baritone's top or three under the bottom, and the
/// notes the singer never sang came out made up.
pub fn fit_key(tune: &[f32], person: (f32, f32)) -> Fit {
    key_costs(tune, person).into_iter().min_by(|a, b| a.1.iter().sum::<f32>().total_cmp(&b.1.iter().sum())).map_or(Fit { voice: 0, band: 0 }, |(f, _)| f)
}

/// Each key `fit_key` weighs, with what it costs: notes sung outside the
/// range, the band moved, and the tune off the range's middle.
pub fn key_costs(tune: &[f32], person: (f32, f32)) -> Vec<(Fit, [f32; 3])> {
    let notes: Vec<f32> = tune.iter().copied().filter(|x| *x > VOICED).map(|hz| 12.0 * (hz / 440.0).log2()).collect();
    if notes.is_empty() || person.0 <= VOICED || person.1 <= person.0 {
        return Vec::new();
    }
    let (low, high) = (12.0 * (person.0 / 440.0).log2(), 12.0 * (person.1 / 440.0).log2());
    let mut sorted = notes.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let tune_middle = sorted[sorted.len() / 2];
    let person_middle = (low + high) / 2.0;
    (-2..=2)
        .flat_map(|k: i32| (-5..=5).map(move |b: i32| (k, b)))
        .map(|(octaves, band)| {
            let t = (12 * octaves + band) as f32;
            let outside = notes.iter().map(|n| ((low - n - t).max(0.0) + (n + t - high).max(0.0)).powi(2)).sum::<f32>() / notes.len() as f32;
            let moved = if band == 0 { 0.0 } else { BAND_MOVED + BAND_MOVE * band.abs() as f32 };
            let off = OFF_MIDDLE * ((tune_middle + t - person_middle).abs() - MIDDLE_EASE).max(0.0);
            (Fit { voice: 12 * octaves + band, band }, [outside, moved, off])
        })
        .collect()
}

/// Semitones (whole octaves) that bring a song's tune nearest the person's
/// voice by its middle, when there's no sung range to fit it to.
pub fn octave_shift(song: f32, person: f32) -> i32 {
    if song <= VOICED || person <= VOICED {
        return 0;
    }
    let octaves = (person / song).log2().round() as i32;
    octaves.clamp(-2, 2) * 12
}

/// `inference.py`'s cross-fade: the end of the last piece out, this one in,
/// on squared cosines.
fn crossfade(prev: &[f32], next: &mut [f32]) {
    let n = prev.len();
    for i in 0..n.min(next.len()) {
        let a = std::f32::consts::FRAC_PI_2 * i as f32 / (n - 1).max(1) as f32;
        let fade_out = a.cos().powi(2);
        let fade_in = (std::f32::consts::FRAC_PI_2 - a).cos().powi(2);
        next[i] = next[i] * fade_in + prev[i] * fade_out;
    }
}

/// Gaussian noise for the flow's start (xorshift and Box-Muller).
pub struct Noise {
    state: u64,
    spare: Option<f32>,
}

impl Noise {
    pub fn new(seed: u64) -> Noise {
        Noise { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, spare: None }
    }

    fn uniform(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        ((self.state >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn normal(&mut self) -> f32 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let (u, v) = (self.uniform(), self.uniform());
        let r = (-2.0 * u.ln()).sqrt();
        let a = std::f32::consts::TAU * v;
        self.spare = Some(r * a.sin());
        r * a.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_are_fitted_to_the_singers_range() {
        let hz = |midi: f32| 440.0 * 2f32.powf((midi - 69.0) / 12.0);
        // A tune: mostly the verse's notes, a few chorus peaks, and rests.
        let tune = |verse: std::ops::Range<i32>, peak: f32| -> Vec<f32> {
            let mut t: Vec<f32> = verse.flat_map(|n| std::iter::repeat_n(hz(n as f32), 20)).collect();
            t.extend(std::iter::repeat_n(hz(peak), 8));
            t.extend([0.0; 30]);
            t
        };
        let baritone = (hz(45.0), hz(63.0));
        // A man's song (verses F#3 to D#4, a chorus peak at G#4) for a
        // baritone (A2 to D#4): the whole song a few semitones down, so its
        // peak is the baritone's top; an octave down would put every verse
        // under the lowest notes.
        let f = fit_key(&tune(54..64, 68.0), baritone);
        assert_eq!(f.voice, f.band, "{f:?}");
        assert!((-5..=-3).contains(&f.band), "{f:?}");
        assert_eq!(octave_shift(hz(56.0), hz(50.0)), -12, "the middles alone went down");
        // A woman (A3 to F5) singing that man's song: an octave up.
        let f = fit_key(&tune(54..64, 66.0), (hz(57.0), hz(77.0)));
        assert_eq!(f.voice - f.band, 12, "{f:?}");
        // A woman's song (A3 to D5) for that baritone: an octave down.
        let f = fit_key(&tune(57..72, 74.0), baritone);
        assert_eq!(f.voice - f.band, -12, "{f:?}");
        // A song already in the middle of the range stays as it is.
        assert_eq!(fit_key(&tune(48..58, 60.0), baritone), Fit { voice: 0, band: 0 });
        assert_eq!(fit_key(&[0.0; 10], baritone), Fit { voice: 0, band: 0 });
    }

    #[test]
    fn a_range_leaves_out_the_odd_note() {
        // Mostly between 100 and 200 Hz, with a crack at 600 and silence.
        let mut f0: Vec<f32> = (0..100).map(|i| 100.0 + i as f32).collect();
        f0.extend([600.0, 0.0, 0.0]);
        let (lo, hi) = range_of(&f0).unwrap();
        assert!((104.0..=106.0).contains(&lo) && (194.0..=196.0).contains(&hi), "{lo} {hi}");
        assert_eq!(range_of(&[0.0; 50]), None);
        assert_eq!((note_name(440.0).as_str(), note_name(130.81).as_str(), note_name(277.18).as_str()), ("A4", "C3", "C#4"));
    }

    #[test]
    fn nearest_frames_follow_torch() {
        // F.interpolate(torch.arange(5.)[None, None], size=8, mode="nearest")
        assert_eq!(nearest(5, 8), vec![0, 0, 1, 1, 2, 3, 3, 4]);
        assert_eq!(nearest(8, 3), vec![0, 2, 5]);
    }

    #[test]
    fn pitch_is_read_from_the_peak() {
        let mut row = vec![0f32; 360];
        row[100] = 0.9;
        let f = decode_pitch(&row, 0.03)[0];
        // Bin 100: 2000 + 1997.38 cents above 10 Hz.
        let want = 10.0 * 2f32.powf((20.0 * 100.0 + 1997.379_4) / 1200.0);
        assert!((f - want).abs() < 0.01, "{f} vs {want}");
        assert_eq!(decode_pitch(&vec![0.01; 360], 0.03), vec![0.0]);
    }

    #[test]
    fn songs_move_by_whole_octaves_toward_the_voice() {
        assert_eq!(octave_shift(294.0, 120.0), -12);
        assert_eq!(octave_shift(150.0, 160.0), 0);
        assert_eq!(octave_shift(110.0, 240.0), 12);
        assert_eq!(octave_shift(0.0, 240.0), 0);
    }

    /// A .npy file's values (float32 or float64, little-endian).
    fn npy(path: &Path) -> Vec<f32> {
        let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let header_len = u16::from_le_bytes([b[8], b[9]]) as usize;
        let header = String::from_utf8_lossy(&b[10..10 + header_len]);
        let data = &b[10 + header_len..];
        if header.contains("'<f8'") {
            data.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap()) as f32).collect()
        } else {
            assert!(header.contains("'<f4'"), "{header}");
            data.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
        }
    }

    fn compare(label: &str, got: &[f32], want: &[f32]) -> f32 {
        assert_eq!(got.len(), want.len(), "{label}: length");
        let mean_err = got.iter().zip(want).map(|(a, b)| (a - b).abs()).sum::<f32>() / got.len() as f32;
        let scale = want.iter().map(|v| v.abs()).sum::<f32>() / want.len() as f32;
        println!("{label}: mean error {mean_err:.3e} against mean size {scale:.3e} ({:.2e})", mean_err / scale.max(1e-9));
        mean_err / scale.max(1e-9)
    }

    /// The ONNX Runtime the installed app downloaded (the graphics card's
    /// when SULCUSAI_ONNX_CARD is set), and a card if asked for; or the one
    /// SULCUSAI_ONNX_DLL names, on the card through DirectML when
    /// SULCUSAI_ONNX_DML is set.
    fn runtime() -> (PathBuf, Option<crate::natural::Card>) {
        let dml = std::env::var_os("SULCUSAI_ONNX_DML").is_some();
        if let Some(dll) = std::env::var_os("SULCUSAI_ONNX_DLL") {
            let card = dml || std::env::var_os("SULCUSAI_ONNX_CARD").is_some();
            return (dll.into(), card.then_some(crate::natural::Card { device: 0, dml }));
        }
        let engines = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("app.sulcusai.desktop").join("engines");
        let card = std::env::var_os("SULCUSAI_ONNX_CARD").is_some();
        let dir = engines.join(if card { "onnxruntime-1.30.0-cuda13" } else { "onnxruntime-1.30.0-cpu-x64" });
        let dll = [dir.join("onnxruntime.dll"), dir.join("onnxruntime-win-x64-1.30.0").join("lib").join("onnxruntime.dll")]
            .into_iter()
            .find(|p| p.is_file())
            .expect("no ONNX Runtime in the installed app's engines");
        (dll, card.then_some(crate::natural::Card { device: 0, dml: false }))
    }

    /// Stop, pressed while a song is being sung again, ends it within a step
    /// (on the processor, where a step is slowest), not at the end of a
    /// 30 s window.
    ///     cargo test stopping_a_conversion -- --ignored --nocapture
    #[test]
    #[ignore]
    fn stopping_a_conversion_is_quick() {
        let dir = PathBuf::from(std::env::var("SULCUSAI_SVC_DIR").unwrap_or_else(|_| r"C:\dev\tools\voice\onnx16".into()));
        let pcm = crate::audio::wav_decode(&std::fs::read(r"C:\dev\tools\voice\tests\own-words-stems\vocals.wav").unwrap()).unwrap();
        let sung = stems::resample(&pcm.samples, pcm.rate, RATE);
        let (dll, card) = runtime();
        let mut conv = Converter::new(&dll, &dir, 8, card).unwrap();
        let voice = conv.voice(&sung[..RATE as usize * 10]).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let (flag, started) = (stop.clone(), std::time::Instant::now());
        let pressed = std::sync::Arc::new(std::sync::Mutex::new(None));
        let at = pressed.clone();
        let progress = move |f: f64| {
            // Pressed once the steps are under way.
            if f > 0.12 && at.lock().unwrap().is_none() {
                *at.lock().unwrap() = Some(std::time::Instant::now());
                flag.store(true, Ordering::Relaxed);
            }
        };
        let r = conv.convert(&voice, &sung, Settings { steps: 10, ..Default::default() }, &stop, &progress);
        let after = pressed.lock().unwrap().map(|t| t.elapsed().as_secs_f64()).unwrap();
        println!("stopped {after:.1} s after Stop ({:.0} s in all)", started.elapsed().as_secs_f64());
        assert_eq!(r.unwrap_err(), crate::download::CANCELLED);
        assert!(after < 15.0);
    }

    /// The pitch of a song longer than a piece, found 30 s at a time, against
    /// one pass over all of it (on the processor): the same frames, voiced
    /// in the same places, within a few cents.
    ///     cargo test long_songs_pitch_in_pieces -- --ignored --nocapture
    #[test]
    #[ignore]
    fn long_songs_pitch_in_pieces_as_in_one_pass() {
        let dir = PathBuf::from(std::env::var("SULCUSAI_SVC_DIR").unwrap_or_else(|_| r"C:\dev\tools\voice\onnx16".into()));
        let pcm = crate::audio::wav_decode(&std::fs::read(r"C:\dev\tools\voice\tests\own-words-stems\vocals.wav").unwrap()).unwrap();
        let w16 = stems::resample(&pcm.samples, pcm.rate, 16_000);
        assert!(w16.len() > 16_000 * 32, "needs more than one piece: {} s", w16.len() / 16_000);
        let (dll, _) = runtime();
        let mut conv = Converter::new(&dll, &dir, 8, None).unwrap();
        let pieces = conv.pitch(&w16).unwrap();
        let salience = conv.run("rmvpe.onnx", vec![("wave", vec![1, w16.len() as i64], w16.clone())]).unwrap();
        let whole = decode_pitch(&salience, 0.03);
        assert_eq!(pieces.len(), whole.len());
        let both: Vec<(f32, f32)> = pieces.iter().copied().zip(whole.iter().copied()).filter(|(a, b)| *a > VOICED && *b > VOICED).collect();
        let agree = pieces.iter().zip(&whole).filter(|(a, b)| (**a > VOICED) == (**b > VOICED)).count() as f32 / whole.len() as f32;
        let cents = both.iter().map(|(a, b)| (1200.0 * (a / b).log2()).abs()).sum::<f32>() / both.len().max(1) as f32;
        println!("{} frames: voiced the same in {:.1}%, {:.1} cents apart on average", whole.len(), agree * 100.0, cents);
        assert!(agree > 0.98 && cents < 10.0);
        // Asked again for the same audio, it isn't worked out again.
        let t = std::time::Instant::now();
        assert_eq!(conv.pitch(&w16).unwrap(), pieces);
        assert!(t.elapsed().as_millis() < 50);
    }

    /// The Rust side against Seed-VC's own run (`export_svc.py`'s saved
    /// values): each piece from the same inputs, and the sampling from the
    /// same noise.
    ///     SULCUSAI_SVC_DIR=C:\dev\tools\voice\onnx16 cargo test seed_vc_matches -- --ignored --nocapture
    #[test]
    #[ignore]
    fn seed_vc_matches_its_own_run() {
        let dir = PathBuf::from(std::env::var("SULCUSAI_SVC_DIR").unwrap_or_else(|_| r"C:\dev\tools\voice\onnx16".into()));
        let r = |n: &str| npy(&dir.join("ref").join(format!("{n}.npy")));
        let (dll, card) = runtime();
        let mut conv = Converter::new(&dll, &dir, 8, card).unwrap();
        let (src16, ref16, src44, ref44) = (r("src16"), r("ref16"), r("src44"), r("ref44"));
        let (n_alt, s_alt) = conv.content(&src16).unwrap();
        assert!(compare("content", &s_alt, &r("S_alt")) < 0.01);
        let (n_ori, s_ori) = conv.content(&ref16).unwrap();
        let f0_alt = conv.pitch(&src16).unwrap();
        assert!(compare("pitch", &f0_alt, &r("F0_alt")) < 0.01);
        let f0_ori = conv.pitch(&ref16).unwrap();
        let (fb_frames, fb) = crate::diarize::fbank_with(&ref16, true);
        assert!(compare("filterbank", &fb, &r("fbank_ref")) < 0.02);
        let style = conv.run("campplus.onnx", vec![("fbank", vec![1, fb_frames as i64, 80], fb)]).unwrap();
        assert!(compare("voice print", &style, &r("style2")) < 0.02);
        let mel2 = conv.run("mel44.onnx", vec![("wave", vec![1, ref44.len() as i64], ref44.clone())]).unwrap();
        assert!(compare("prompt mel", &mel2, &r("mel2")) < 1e-3);
        let frames_src = conv.run("mel44.onnx", vec![("wave", vec![1, src44.len() as i64], src44.clone())]).unwrap().len() / MELS;
        let frames_ref = mel2.len() / MELS;
        let cond = conv.regulate(&s_alt, n_alt, &f0_alt, frames_src).unwrap();
        assert!(compare("condition", &cond, &r("cond")) < 0.02);
        let prompt = conv.regulate(&s_ori, n_ori, &f0_ori, frames_ref).unwrap();
        assert!(compare("prompt condition", &prompt, &r("prompt_cond")) < 0.02);
        let voice = Voice { mel: mel2, frames: frames_ref, condition: prompt, style, pitch: middle_pitch(&f0_ori) };
        let s = Settings { steps: 10, ..Default::default() };
        let started = std::time::Instant::now();
        let mel = conv.sample(&voice, &cond, frames_src, s, r("z"), &AtomicBool::new(false), &|_| {}).unwrap();
        println!("10 steps over {} frames in {:.1} s ({})", frames_ref + frames_src, started.elapsed().as_secs_f64(), if conv.on_card { "card" } else { "processor" });
        let want = r("vc_mel");
        let want_src: Vec<f32> = (0..MELS).flat_map(|m| want[m * (frames_ref + frames_src) + frames_ref..(m + 1) * (frames_ref + frames_src)].to_vec()).collect();
        assert!(compare("sampled mel", &mel, &want_src) < 0.05);
        let wave = conv.run("bigvgan.onnx", vec![("mel", vec![1, MELS as i64, frames_src as i64], mel)]).unwrap();
        assert!(compare("sound", &wave, &r("wave")) < 0.1);
    }

    /// Singing found with silence around it: a long break splits it, a short
    /// one doesn't, and half a second is kept either side.
    #[test]
    fn only_the_sung_stretches_are_worked_on() {
        let tone = |s: f32| (0..(s * RATE as f32) as usize).map(|i| (i as f32 * 0.05).sin() * 0.3).collect::<Vec<f32>>();
        let quiet = |s: f32| vec![0f32; (s * RATE as f32) as usize];
        let parts = [quiet(2.0), tone(3.0), quiet(5.0), tone(2.0), quiet(1.0), tone(1.0), quiet(3.0)];
        let sung: Vec<f32> = parts.concat();
        let frames = sung.len() / HOP + 1;
        let secs = |f: usize| f as f32 * HOP as f32 / RATE as f32;
        let found: Vec<(f32, f32)> = singing(&sung, frames).into_iter().map(|(a, b)| (secs(a), secs(b))).collect();
        println!("{found:?}");
        assert_eq!(found.len(), 2);
        assert!((found[0].0 - 1.5).abs() < 0.05 && (found[0].1 - 5.5).abs() < 0.05);
        assert!((found[1].0 - 9.5).abs() < 0.05 && (found[1].1 - 14.5).abs() < 0.05);
        assert!(singing(&quiet(3.0), 3 * RATE as usize / HOP).is_empty());
    }

    #[test]
    fn noise_is_standard_normal() {
        let mut n = Noise::new(3);
        let v: Vec<f32> = (0..20_000).map(|_| n.normal()).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32;
        assert!(mean.abs() < 0.03 && (var - 1.0).abs() < 0.05, "{mean} {var}");
    }
}

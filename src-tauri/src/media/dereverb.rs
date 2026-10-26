// SPDX-License-Identifier: AGPL-3.0-only
//! Remove FX (Suno Studio's): a sound's reverb and echo taken out by
//! anvuew's de-reverb Mel-Band RoFormer (GPL-3.0; trained on dry single
//! voices, so best on a vocal), on ONNX Runtime. The ONNX file is the network
//! between its STFT and inverse STFT (exported with audiojs's
//! export-roformer.py), so both are done here as the model was trained:
//! n_fft 2048, hop 441, a Hann window, centred with reflected edges, in 8 s
//! pieces every 4 s, cross-faded (its trainer's `demix`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::Session;
use ort::value::Tensor;
use realfft::RealFftPlanner;

use super::stems::Stereo;

pub const RATE: u32 = 44_100;
const SEGMENT: usize = 352_800;
const STEP: usize = SEGMENT / 2;
const N_FFT: usize = 2048;
const HOP: usize = 441;
const BINS: usize = N_FFT / 2 + 1;
const FRAMES: usize = SEGMENT / HOP + 1;

fn e(err: impl std::fmt::Display) -> String {
    format!("Removing the effects failed: {err}")
}

fn session() -> &'static Mutex<Option<(PathBuf, bool, Session)>> {
    static S: OnceLock<Mutex<Option<(PathBuf, bool, Session)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn hann() -> Vec<f32> {
    // Periodic, as torch.hann_window makes it.
    (0..N_FFT).map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / N_FFT as f64).cos()) as f32).collect()
}

/// `x` reflected `pad` samples out at both ends (as torch pads for a centred STFT).
fn reflect(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    let at = |i: isize| -> f32 {
        let mut j = i;
        // Reflect without repeating the edge sample, as many times as needed.
        loop {
            if j < 0 {
                j = -j;
            } else if j >= n as isize {
                j = 2 * (n as isize - 1) - j;
            } else {
                return x[j as usize];
            }
            if n == 1 {
                return x[0];
            }
        }
    };
    (-(pad as isize)..(n + pad) as isize).map(at).collect()
}

/// One 8 s stereo piece as the model's input: [L re, L im, R re, R im] × 1025 bins × 801 frames.
fn stft(piece: [&[f32]; 2], window: &[f32], planner: &mut RealFftPlanner<f32>) -> Vec<f32> {
    let fft = planner.plan_fft_forward(N_FFT);
    let mut out = vec![0f32; 4 * BINS * FRAMES];
    let mut buf = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    for (c, x) in piece.iter().enumerate() {
        let padded = reflect(x, N_FFT / 2);
        for t in 0..FRAMES {
            let a = t * HOP;
            for n in 0..N_FFT {
                buf[n] = padded[a + n] * window[n];
            }
            fft.process(&mut buf, &mut spec).map_err(e).ok();
            for (f, z) in spec.iter().enumerate() {
                out[((2 * c) * BINS + f) * FRAMES + t] = z.re;
                out[((2 * c + 1) * BINS + f) * FRAMES + t] = z.im;
            }
        }
    }
    out
}

/// The model's output back to sound: overlap-added, divided by the window's
/// summed square, the centring padding cut off (torch.istft's way).
fn istft(spec: &[f32], window: &[f32], planner: &mut RealFftPlanner<f32>) -> [Vec<f32>; 2] {
    let ifft = planner.plan_fft_inverse(N_FFT);
    let total = N_FFT + (FRAMES - 1) * HOP;
    let mut env = vec![0f32; total];
    for t in 0..FRAMES {
        for n in 0..N_FFT {
            env[t * HOP + n] += window[n] * window[n];
        }
    }
    let mut z = ifft.make_input_vec();
    let mut frame = ifft.make_output_vec();
    let mut out = [vec![0f32; SEGMENT], vec![0f32; SEGMENT]];
    for (c, dst) in out.iter_mut().enumerate() {
        let mut acc = vec![0f32; total];
        for t in 0..FRAMES {
            for (f, v) in z.iter_mut().enumerate() {
                v.re = spec[((2 * c) * BINS + f) * FRAMES + t];
                v.im = if f == 0 || f == BINS - 1 { 0.0 } else { spec[((2 * c + 1) * BINS + f) * FRAMES + t] };
            }
            ifft.process(&mut z, &mut frame).map_err(e).ok();
            for n in 0..N_FFT {
                acc[t * HOP + n] += frame[n] / N_FFT as f32 * window[n];
            }
        }
        for (i, v) in dst.iter_mut().enumerate() {
            let k = i + N_FFT / 2;
            *v = if env[k] > 1e-11 { acc[k] / env[k] } else { 0.0 };
        }
    }
    out
}

/// The sound with its reverb and echo taken out (44.1 kHz stereo in and
/// out, the same length); and whether the graphics card did it. A piece is
/// 8 s, one every 4 s, the ends reflected out so they're worked on whole,
/// each faded in and out over 0.8 s where it overlaps another. Stopping
/// (`cancel`) is checked at each piece.
pub fn dry(dll: &Path, model: &Path, sound: &Stereo, threads: usize, card: Option<crate::natural::Card>, cancel: &std::sync::atomic::AtomicBool, progress: &dyn Fn(f64)) -> Result<(Stereo, bool), String> {
    crate::natural::runtime_ready(dll)?;
    let mut guard = session().lock().unwrap();
    if guard.as_ref().map(|(p, on, _)| (p.as_path(), *on)) != Some((model, card.is_some())) {
        let (s, on) = crate::natural::session_on(model, threads, card)?;
        *guard = Some((model.to_path_buf(), on, s));
    }
    let n = sound.len();
    // The ends reflected out by what an overlap misses, so every sample is
    // in two pieces (as the trainer's own `demix`), when it's long enough.
    let border = SEGMENT - STEP;
    let pad = if n > 2 * border { border } else { 0 };
    let x = [0, 1].map(|c| if pad > 0 { reflect(&sound.ch[c], pad) } else { sound.ch[c].clone() });
    let len = x[0].len();
    let fade = SEGMENT / 10;
    let starts: Vec<usize> = (0..len).step_by(STEP).collect();
    let mut out = [vec![0f32; len], vec![0f32; len]];
    let mut weight = vec![0f32; len];
    let window = hann();
    let mut planner = RealFftPlanner::<f32>::new();
    for (k, &a) in starts.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        progress(k as f64 / starts.len() as f64);
        let b = (a + SEGMENT).min(len);
        let piece = [0, 1].map(|c| {
            let mut p = vec![0f32; SEGMENT];
            p[..b - a].copy_from_slice(&x[c][a..b]);
            p
        });
        let input = stft([&piece[0], &piece[1]], &window, &mut planner);
        let spec = loop {
            let (_, on, sess) = guard.as_mut().unwrap();
            let ran = sess
                .run(ort::inputs! { "mix_spec" => Tensor::from_array(([1usize, 4, BINS, FRAMES], input.clone())).map_err(e)? })
                .and_then(|res| Ok(res["stems_spec"].try_extract_tensor::<f32>()?.1.to_vec()));
            match ran {
                Ok(v) => break v,
                Err(err) if *on => {
                    crate::applog::line("media", &format!("The graphics card couldn't remove the effects ({err}); going on on the processor"));
                    let (s, _) = crate::natural::session_on(model, threads, None)?;
                    *guard = Some((model.to_path_buf(), false, s));
                }
                Err(err) => return Err(e(err)),
            }
        };
        if spec.len() != 4 * BINS * FRAMES {
            return Err(e("unexpected output size"));
        }
        let y = istft(&spec, &window, &mut planner);
        let first = k == 0;
        let last = b == len;
        for i in 0..b - a {
            let mut w = 1f32;
            if !first && i < fade {
                w = i as f32 / fade as f32;
            }
            if !last && i >= SEGMENT - fade {
                w = w.min((SEGMENT - i) as f32 / fade as f32);
            }
            out[0][a + i] += y[0][i] * w;
            out[1][a + i] += y[1][i] * w;
            weight[a + i] += w;
        }
        if last {
            break;
        }
    }
    let ch = [0, 1].map(|c| (pad..pad + n).map(|i| out[c][i] / weight[i].max(1e-8)).collect::<Vec<f32>>());
    let on_card = guard.as_ref().is_some_and(|(_, on, _)| *on);
    Ok((Stereo { rate: RATE, ch }, on_card))
}

pub fn unload() {
    *session().lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transform_goes_there_and_back() {
        let tone: Vec<f32> = (0..SEGMENT).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin() * 0.5 + ((i * 7919) % 1000) as f32 / 10_000.0).collect();
        let window = hann();
        let mut planner = RealFftPlanner::<f32>::new();
        let spec = stft([&tone, &tone], &window, &mut planner);
        assert_eq!(spec.len(), 4 * BINS * FRAMES);
        // 440 Hz is bin 20.4 of 1025: the strongest bin of a middle frame.
        let mag = |f: usize| (spec[f * FRAMES + 400].powi(2) + spec[(BINS + f) * FRAMES + 400].powi(2)).sqrt();
        let top = (1..BINS).max_by(|a, b| mag(*a).total_cmp(&mag(*b))).unwrap();
        assert!((20..=21).contains(&top), "{top}");
        let back = istft(&spec, &window, &mut planner);
        let worst = back[0].iter().zip(&tone).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn edges_are_reflected_without_repeating() {
        assert_eq!(reflect(&[1.0, 2.0, 3.0], 2), vec![3.0, 2.0, 1.0, 2.0, 3.0, 2.0, 1.0]);
    }
}

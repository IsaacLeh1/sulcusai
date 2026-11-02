// SPDX-License-Identifier: AGPL-3.0-only
//! A recording moved up or down in pitch at the same length, so a song can
//! be put in a singer's key: stretched in time by a phase vocoder, then
//! played faster or slower by rate conversion. The phases are locked to the
//! nearest peak (Laroche and Dolson's identity locking), which keeps chords
//! from sounding washed out, and both channels turn by the same amount, so
//! the stereo picture stays where it was.

use std::f32::consts::TAU;

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use super::stems::{self, Stereo};

const N: usize = 4096;
const HOP: usize = N / 4;

/// `x` moved by `semitones`, the same length and rate.
pub fn shift(x: &Stereo, semitones: i32) -> Stereo {
    if semitones == 0 || x.len() == 0 {
        return x.clone();
    }
    // The ratio as two rates sharing a factor of 100, so the converter's
    // table stays small; within a cent of the semitone, and the stretch uses
    // the same ratio, so the length (and the beat) comes out exact.
    let to = 44_100u32;
    let from = (2f64.powf(semitones as f64 / 12.0) * 441.0).round() as u32 * 100;
    let long = stretch(&x.ch, from as f64 / to as f64);
    let ch = long.map(|c| {
        let mut out = stems::resample(&c, from, to);
        out.resize(x.len(), 0.0);
        out
    });
    Stereo { rate: x.rate, ch }
}

fn wrap(a: f32) -> f32 {
    a - TAU * (a / TAU).round()
}

/// Both channels `alpha` times as long, at the same pitch.
fn stretch(ch: &[Vec<f32>; 2], alpha: f64) -> [Vec<f32>; 2] {
    let len = ch[0].len();
    let want = (len as f64 * alpha).round() as usize;
    let ha = HOP as f64 / alpha;
    let pad = N / 2;
    let frames = (want + pad) / HOP + 2;
    let need = (((frames - 1) as f64 * ha).round() as usize) + N;
    let padded: [Vec<f32>; 2] = std::array::from_fn(|c| {
        let mut p = vec![0f32; pad];
        p.extend_from_slice(&ch[c]);
        p.resize(need.max(p.len()), 0.0);
        p
    });
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(N);
    let inv = planner.plan_fft_inverse(N);
    let window: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (TAU * i as f32 / N as f32).cos()).collect();
    let bins = N / 2 + 1;
    // A Hann window in and out at a quarter-window hop adds up to 1.5.
    let scale = 1.0 / (N as f32 * 1.5);
    let mut out = [vec![0f32; (frames - 1) * HOP + N], vec![0f32; (frames - 1) * HOP + N]];
    let mut buf = fwd.make_input_vec();
    let mut spec = [fwd.make_output_vec(), fwd.make_output_vec()];
    let mut prev_phase = vec![0f32; bins];
    let mut out_phase = vec![0f32; bins];
    let mut last_pos = 0usize;
    for k in 0..frames {
        let pos = (k as f64 * ha).round() as usize;
        for c in 0..2 {
            for i in 0..N {
                buf[i] = padded[c][pos + i] * window[i];
            }
            let _ = fwd.process(&mut buf, &mut spec[c]);
        }
        // The phases are worked out on both channels together.
        let both: Vec<Complex<f32>> = (0..bins).map(|b| spec[0][b] + spec[1][b]).collect();
        let mag: Vec<f32> = both.iter().map(|z| z.norm()).collect();
        let phase: Vec<f32> = both.iter().map(|z| z.arg()).collect();
        if k == 0 {
            out_phase.copy_from_slice(&phase);
        } else {
            let advance = (pos - last_pos) as f32;
            let peaks: Vec<usize> = (0..bins).filter(|&b| mag[b] > 0.0 && (b.saturating_sub(2)..=(b + 2).min(bins - 1)).all(|j| j == b || mag[j] < mag[b])).collect();
            let mut next = vec![0f32; bins];
            for &p in &peaks {
                let omega = TAU * p as f32 / N as f32;
                let freq = omega + wrap(phase[p] - prev_phase[p] - omega * advance) / advance.max(1.0);
                next[p] = out_phase[p] + freq * HOP as f32;
            }
            if peaks.is_empty() {
                for b in 0..bins {
                    let omega = TAU * b as f32 / N as f32;
                    next[b] = out_phase[b] + omega * HOP as f32;
                }
            } else {
                let mut at = 0;
                for b in 0..bins {
                    while at + 1 < peaks.len() && peaks[at + 1].abs_diff(b) <= peaks[at].abs_diff(b) {
                        at += 1;
                    }
                    let p = peaks[at];
                    if p != b {
                        next[b] = next[p] + phase[b] - phase[p];
                    }
                }
            }
            out_phase = next;
        }
        prev_phase.copy_from_slice(&phase);
        last_pos = pos;
        for c in 0..2 {
            for b in 0..bins {
                spec[c][b] *= Complex::from_polar(1.0, out_phase[b] - phase[b]);
            }
            spec[c][0].im = 0.0;
            spec[c][bins - 1].im = 0.0;
            let _ = inv.process(&mut spec[c], &mut buf);
            let at = k * HOP;
            for i in 0..N {
                out[c][at + i] += buf[i] * window[i] * scale;
            }
        }
    }
    out.map(|mut o| {
        o.drain(..pad.min(o.len()));
        o.resize(want, 0.0);
        o
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        (0..(44_100.0 * secs) as usize).map(|i| 0.5 * (TAU * hz * i as f32 / 44_100.0).sin()).collect()
    }

    /// The strongest frequency over the middle second, by zero crossings.
    fn pitch(x: &[f32]) -> f32 {
        let mid = &x[x.len() / 2 - 22_050..x.len() / 2 + 22_050];
        let crossings = mid.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f32
    }

    #[test]
    fn a_tone_moves_by_the_semitones_and_keeps_its_length() {
        let a = tone(220.0, 3.0);
        let x = Stereo { rate: 44_100, ch: [a.clone(), a] };
        for (semis, hz) in [(-5, 164.81), (3, 261.63), (12, 440.0)] {
            let y = shift(&x, semis);
            assert_eq!(y.len(), x.len());
            let got = pitch(&y.ch[0]);
            assert!((got - hz).abs() < 2.0, "{semis}: {got} Hz, wanted {hz}");
            assert_eq!(pitch(&y.ch[1]), got);
        }
    }

    #[test]
    fn the_loudness_stays() {
        let a = tone(330.0, 3.0);
        let x = Stereo { rate: 44_100, ch: [a.clone(), a] };
        let y = shift(&x, -4);
        let mid = |v: &[f32]| crate::audio::rms(&v[v.len() / 4..v.len() * 3 / 4]);
        let ratio = mid(&y.ch[0]) / mid(&x.ch[0]);
        assert!((0.85..1.15).contains(&ratio), "{ratio}");
    }

    #[test]
    fn no_shift_is_the_same_sound() {
        let a = tone(330.0, 1.0);
        let x = Stereo { rate: 44_100, ch: [a.clone(), a] };
        assert_eq!(shift(&x, 0), x);
    }
}

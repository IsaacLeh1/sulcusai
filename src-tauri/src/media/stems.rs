// SPDX-License-Identifier: AGPL-3.0-only
//! A song's parts (drums, bass, other instruments, vocals), taken apart by
//! HTDemucs (MIT, on ONNX Runtime), for the song view (`songmap`); and
//! clean rate conversion between 48 and 44.1 kHz.
//!
//! The parts are only read. Putting a changed vocal part back into a song
//! (breaths remade, the voice brought up) was tried and taken out: the
//! separated voice carries watery leftovers and some of the band, and mixed
//! back in it gave high-pitched hits, a feedback-like feel, a voice that
//! sounded more buried and brief dips at each breath. The songs as the
//! music model made them sounded better.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::Session;
use ort::value::Tensor;

/// Stereo samples, one list a channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Stereo {
    pub rate: u32,
    pub ch: [Vec<f32>; 2],
}

impl Stereo {
    pub fn len(&self) -> usize {
        self.ch[0].len()
    }

    pub fn mono(&self) -> Vec<f32> {
        self.ch[0].iter().zip(&self.ch[1]).map(|(l, r)| 0.5 * (l + r)).collect()
    }

    pub fn resampled(&self, to: u32) -> Stereo {
        Stereo { rate: to, ch: [resample(&self.ch[0], self.rate, to), resample(&self.ch[1], self.rate, to)] }
    }
}

/// HTDemucs' rate and the length of audio it takes at once (7.8 s).
pub const RATE: u32 = 44_100;
const SEP_LEN: usize = 343_980;

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The zeroth-order modified Bessel function (Kaiser windows).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}

/// Rate conversion by a rational ratio with a Kaiser-windowed sinc, 32 zero
/// crossings a side: clean for music, both ways between 48 and 44.1 kHz.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let g = gcd(from, to);
    let (up, down) = ((to / g) as usize, (from / g) as usize);
    let cutoff = 0.95 * (up as f64 / down as f64).min(1.0);
    let half = (32.0 / cutoff).ceil() as isize;
    let taps = (2 * half + 1) as usize;
    let beta = 8.6;
    let i0b = bessel_i0(beta);
    // One row of taps for each of the `up` places an output falls between inputs.
    let mut table = vec![0f32; up * taps];
    for p in 0..up {
        let frac = p as f64 / up as f64;
        let row = &mut table[p * taps..(p + 1) * taps];
        let mut sum = 0.0;
        let mut vals = vec![0f64; taps];
        for (j, v) in vals.iter_mut().enumerate() {
            let t = frac - (j as isize - half) as f64;
            let u = t / (half as f64 + 1.0);
            let w = if u.abs() >= 1.0 { 0.0 } else { bessel_i0(beta * (1.0 - u * u).sqrt()) / i0b };
            let a = std::f64::consts::PI * cutoff * t;
            let s = if a.abs() < 1e-12 { 1.0 } else { a.sin() / a };
            *v = s * w;
            sum += *v;
        }
        for (c, v) in row.iter_mut().zip(&vals) {
            *c = (v / sum) as f32;
        }
    }
    let out_len = (x.len() as u64 * up as u64 / down as u64) as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let pos = n * down;
        let (i, p) = ((pos / up) as isize, pos % up);
        let row = &table[p * taps..(p + 1) * taps];
        let start = i - half;
        let acc = if start >= 0 && start as usize + taps <= x.len() {
            x[start as usize..start as usize + taps].iter().zip(row).map(|(a, b)| a * b).sum()
        } else {
            row.iter().enumerate().filter_map(|(j, c)| usize::try_from(start + j as isize).ok().and_then(|k| x.get(k)).map(|v| v * c)).sum()
        };
        out.push(acc);
    }
    out
}

/// The open separator: its file, and whether it's on the graphics card.
fn session() -> &'static Mutex<Option<(PathBuf, bool, Session)>> {
    static S: OnceLock<Mutex<Option<(PathBuf, bool, Session)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn e(err: impl std::fmt::Display) -> String {
    format!("Taking the song apart failed: {err}")
}

/// HTDemucs' parts, in the order it gives them.
pub const PARTS: [&str; 4] = ["drums", "bass", "other", "vocals"];

/// Parts of a 44.1 kHz song (`wanted`, indexes into `PARTS`), by HTDemucs
/// in 7.8 s pieces overlapping by a quarter and cross-faded (its own
/// reference program's way); and whether the graphics card did it.
///
/// On an NVIDIA card (`card`) a piece took 0.37 s (RTX 5070), on the
/// processor 1.8 s (i5-14600K); the card's first piece takes 1 s (8.5 the
/// first time ever, while CUDA builds its kernels). A 2:41 song came apart
/// in 17 s on the card and 41-84 s on the processor. Should the card fail,
/// it goes on on the processor.
/// Stopping (`cancel`) is checked at each piece.
pub fn separate(dll: &Path, model: &Path, mix: &Stereo, threads: usize, wanted: &[usize], card: Option<crate::natural::Card>, cancel: &std::sync::atomic::AtomicBool) -> Result<(Vec<Stereo>, bool), String> {
    crate::natural::runtime_ready(dll)?;
    // Not through DirectML: HTDemucs failed there at once ("not enough
    // memory resources", with the card's memory barely used), and without
    // fusing its graph a 7.8 s piece took 8 minutes (RTX 5070).
    let card = card.filter(|c| !c.dml);
    let mut guard = session().lock().unwrap();
    if guard.as_ref().map(|(p, on, _)| (p.as_path(), *on)) != Some((model, card.is_some())) {
        let (s, on) = crate::natural::session_on(model, threads, card)?;
        *guard = Some((model.to_path_buf(), on, s));
    }
    let total = mix.len();
    let overlap = SEP_LEN / 4;
    let stride = SEP_LEN - overlap;
    let mut win = vec![1f32; SEP_LEN];
    for k in 0..overlap {
        let v = k as f32 / (overlap - 1) as f32;
        win[k] = v;
        win[SEP_LEN - 1 - k] = v;
    }
    let mut out: Vec<[Vec<f32>; 2]> = wanted.iter().map(|_| [vec![0f32; total], vec![0f32; total]]).collect();
    let mut weight = vec![0f32; total];
    let mut input = vec![0f32; 2 * SEP_LEN];
    for i in 0..total.div_ceil(stride).max(1) {
        let a = i * stride;
        if a >= total {
            break;
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        let b = (a + SEP_LEN).min(total);
        input.iter_mut().for_each(|v| *v = 0.0);
        for c in 0..2 {
            input[c * SEP_LEN..c * SEP_LEN + (b - a)].copy_from_slice(&mix.ch[c][a..b]);
        }
        let stems = loop {
            let (_, on, sess) = guard.as_mut().unwrap();
            let ran = sess
                .run(ort::inputs! { "mix" => Tensor::from_array(([1usize, 2, SEP_LEN], input.clone())).map_err(e)? })
                .and_then(|res| Ok(res["stems"].try_extract_tensor::<f32>()?.1.to_vec()));
            match ran {
                Ok(v) => break v,
                Err(err) if *on => {
                    let why = format!("the graphics card failed partway ({})", err.to_string().lines().next().unwrap_or(""));
                    if !crate::cardfirst::go_on_on_processor("Taking the song apart", &why) {
                        return Err(crate::download::CANCELLED.into());
                    }
                    crate::applog::line("media", &format!("The graphics card couldn't take the song apart ({err}); going on on the processor"));
                    let (s, _) = crate::natural::session_on(model, threads, None)?;
                    *guard = Some((model.to_path_buf(), false, s));
                }
                Err(err) => return Err(e(err)),
            }
        };
        if stems.len() != 4 * 2 * SEP_LEN {
            return Err(e("unexpected output size"));
        }
        for (part, into) in wanted.iter().zip(out.iter_mut()) {
            let got = &stems[part * 2 * SEP_LEN..(part + 1) * 2 * SEP_LEN];
            for c in 0..2 {
                for k in 0..(b - a) {
                    into[c][a + k] += got[c * SEP_LEN + k] * win[k];
                }
            }
        }
        for k in 0..(b - a) {
            weight[a + k] += win[k];
        }
    }
    for part in &mut out {
        for c in part.iter_mut() {
            for (v, w) in c.iter_mut().zip(&weight) {
                *v /= w.max(1e-8);
            }
        }
    }
    let on_card = guard.as_ref().is_some_and(|(_, on, _)| *on);
    Ok((out.into_iter().map(|ch| Stereo { rate: mix.rate, ch }).collect(), on_card))
}

pub fn unload() {
    *session().lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One piece of HTDemucs through DirectML with each way of setting it up,
    /// with the card's memory watched (DirectML ran out on a 12 GB card).
    ///     SULCUSAI_DML_HOW=fused|unfused|basic cargo test separator_through_directml -- --ignored --nocapture
    #[test]
    #[ignore]
    fn separator_through_directml() {
        let engines = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join(r"app.sulcusai.desktop\engines\onnxruntime-1.24.4-cards");
        crate::natural::runtime_ready(&engines.join("onnxruntime.dll")).unwrap();
        let model = PathBuf::from(r"D:\SulcusAI models\media\helpers\htdemucs_fp16weights.onnx");
        let how = std::env::var("SULCUSAI_DML_HOW").unwrap_or_else(|_| "unfused".into());
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (p2, done) = (peak.clone(), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let d2 = done.clone();
        let watch = std::thread::spawn(move || {
            while !d2.load(std::sync::atomic::Ordering::Relaxed) {
                let o = std::process::Command::new("nvidia-smi").args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"]).output().unwrap();
                let used: u64 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0);
                p2.fetch_max(used, std::sync::atomic::Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
        let dml = ort::ep::DirectML::default().with_device_id(0).build().error_on_failure();
        let mut b = Session::builder().unwrap().with_memory_pattern(false).unwrap().with_parallel_execution(false).unwrap();
        if how == "unfused" {
            b = b.with_config_entry("ep.dml.disable_graph_fusion", "1").unwrap();
        }
        if how == "basic" {
            b = b.with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level1).unwrap();
        }
        let t = std::time::Instant::now();
        let mut s = b.with_execution_providers([dml]).unwrap().commit_from_file(&model).unwrap();
        let opened = t.elapsed().as_secs_f64();
        let input: Vec<f32> = (0..2 * SEP_LEN).map(|i| ((i as f32) * 0.01).sin() * 0.3).collect();
        for round in 0..3 {
            let t = std::time::Instant::now();
            let r = s.run(ort::inputs! { "mix" => Tensor::from_array(([1usize, 2, SEP_LEN], input.clone())).unwrap() }).map(|res| res["stems"].try_extract_tensor::<f32>().unwrap().1.len());
            println!("{how}: piece {round}: {:?} in {:.2} s (opened in {opened:.1} s)", r.as_ref().map_err(|e| e.to_string().chars().take(200).collect::<String>()), t.elapsed().as_secs_f64());
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        watch.join().unwrap();
        println!("{how}: the card's memory peaked at {} MiB", peak.load(std::sync::atomic::Ordering::Relaxed));
    }

    fn tone(rate: u32, secs: f32, hz: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn db(rms: f32) -> f32 {
        20.0 * (rms + 1e-9).log10()
    }

    #[test]
    fn resampling_keeps_a_tone_clean() {
        let x = tone(48_000, 1.0, 1000.0);
        let y = resample(&x, 48_000, 44_100);
        assert_eq!(y.len(), 44_100);
        let want = tone(44_100, 1.0, 1000.0);
        // Away from the ends, within -60 dB of the ideal tone.
        let err = rms(&y[2000..42_000].iter().zip(&want[2000..42_000]).map(|(a, b)| a - b).collect::<Vec<_>>());
        assert!(db(err) - db(rms(&want)) < -60.0, "{}", db(err) - db(rms(&want)));
        // And back again.
        let z = resample(&y, 44_100, 48_000);
        assert_eq!(z.len(), 48_000);
        let err = rms(&z[3000..45_000].iter().zip(&x[3000..45_000]).map(|(a, b)| a - b).collect::<Vec<_>>());
        assert!(db(err) - db(rms(&x)) < -55.0);
    }
}

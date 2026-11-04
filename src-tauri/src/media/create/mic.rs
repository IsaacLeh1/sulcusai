// SPDX-License-Identifier: AGPL-3.0-only
//! A mic check (is it loud enough, clipping, noisy, humming) and cleaning
//! recorded speech. Cleaning runs only when asked, on a copy, never by
//! default: the original stays as it was recorded.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Check {
    pub peak_db: f64,
    pub speech_db: f64,
    pub noise_db: f64,
    pub clipped: f64,
    pub hum: bool,
    /// Plain advice lines; empty means it sounds fine.
    pub advice: Vec<String>,
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-9).log10()
}

/// Strength of one frequency in a block (Goertzel).
fn tone(x: &[f32], rate: f64, hz: f64) -> f64 {
    let k = 2.0 * (std::f64::consts::TAU * hz / rate).cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s = v as f64 + k * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - k * s1 * s2).max(0.0).sqrt() / x.len().max(1) as f64 * 2.0
}

/// Checks a few seconds of mono samples (-1..1) of someone speaking.
pub fn check(samples: &[f32], rate: u32) -> Check {
    let block = (rate as usize / 50).max(1); // 20 ms
    let mut levels: Vec<f64> = samples.chunks(block).map(|c| (c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / c.len() as f64).sqrt()).collect();
    levels.sort_by(f64::total_cmp);
    let pick = |q: f64| levels.get(((levels.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    let peak = samples.iter().fold(0.0f64, |m, v| m.max((*v as f64).abs()));
    let clipped = samples.iter().filter(|v| v.abs() >= 0.999).count() as f64 / samples.len().max(1) as f64;
    let (speech_db, noise_db) = (db(pick(0.9)), db(pick(0.1)));
    // Mains hum is 50 or 60 Hz; compare against the quiet level.
    let quiet = pick(0.1);
    let hum = [50.0, 60.0, 100.0, 120.0].iter().any(|&f| tone(samples, rate as f64, f) > (quiet * 0.7).max(0.003));
    let mut advice = Vec::new();
    if samples.is_empty() || peak < 1e-4 {
        advice.push("Nothing was heard. Check the microphone is chosen and not muted.".to_string());
    } else {
        if clipped > 0.001 {
            advice.push("Too loud: some of it is cut off. Turn the input level down or move back.".into());
        } else if speech_db < -30.0 {
            advice.push("Quiet: move closer or turn the input level up.".into());
        }
        if speech_db - noise_db < 15.0 {
            advice.push("Lots of background noise compared with the voice.".into());
        }
        if hum {
            advice.push("A hum from the power line: try another USB port or unplug the charger.".into());
        }
    }
    Check { peak_db: db(peak), speech_db, noise_db, clipped, hum, advice }
}

/// FFmpeg filters for cleaning speech. Gentle by default; `strong` for bad rooms.
pub fn clean_filter(c: &Check, strong: bool) -> String {
    let mut f = vec!["highpass=f=80".to_string()];
    if c.hum {
        f.push("bandreject=f=50:width_type=h:w=4,bandreject=f=60:width_type=h:w=4".into());
    }
    let nr = if strong { 20 } else { 10 };
    f.push(format!("afftdn=nr={nr}:nf={:.0}", c.noise_db.clamp(-80.0, -20.0)));
    f.push("loudnorm=I=-16:TP=-1.5:LRA=11".into());
    f.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn voice(rate: u32, secs: f64, amp: f32, noise: f32) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        let mut seed = 1u32;
        (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let r = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                let t = i as f32 / rate as f32;
                let on = if (t * 2.0) as u32 % 2 == 0 { 1.0 } else { 0.0 };
                amp * on * (t * 220.0 * std::f32::consts::TAU).sin() + noise * r
            })
            .collect()
    }
    #[test]
    fn good_voice_has_no_advice() {
        let c = check(&voice(16000, 3.0, 0.3, 0.001), 16000);
        assert!(c.advice.is_empty(), "{c:?}");
    }
    #[test]
    fn quiet_and_clipped() {
        assert!(check(&voice(16000, 3.0, 0.005, 0.0001), 16000).advice[0].starts_with("Quiet"));
        let loud: Vec<f32> = voice(16000, 3.0, 2.0, 0.0).into_iter().map(|v| v.clamp(-1.0, 1.0)).collect();
        assert!(check(&loud, 16000).advice[0].starts_with("Too loud"));
        assert!(check(&[], 16000).advice[0].starts_with("Nothing"));
    }
    #[test]
    fn hum_found_and_filtered() {
        let mut v = voice(16000, 3.0, 0.3, 0.0);
        for (i, s) in v.iter_mut().enumerate() {
            *s += 0.05 * (i as f32 / 16000.0 * 60.0 * std::f32::consts::TAU).sin();
        }
        let c = check(&v, 16000);
        assert!(c.hum);
        assert!(clean_filter(&c, false).contains("bandreject"));
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Clips longer than a video model makes at once (Wan 2.2: about 5 seconds):
//! made in parts, each starting from the last frame of the one before, then
//! joined into one MP4. Colors drift a little from part to part, so each
//! later part is matched to the first one's.

use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbaImage};

/// The longest clip that can be asked for, in seconds.
pub const MAX_SECS: f32 = 600.0;

/// Frames for each part (each 4k+1, at most `max_frames`): the first makes
/// its frames, every later one repeats the frame it starts from (dropped
/// when joining) and adds the rest. Covers `total` frames, maybe a few more.
pub fn plan(total: u32, max_frames: u32) -> Vec<u32> {
    let up = |n: u32| n.saturating_sub(1).div_ceil(4) * 4 + 1;
    let max_frames = ((max_frames.max(17) - 1) / 4) * 4 + 1;
    if total <= max_frames {
        return vec![up(total.max(5))];
    }
    let mut parts = vec![max_frames];
    let mut need = total - max_frames;
    while need > 0 {
        // At least 17 frames a part: shorter ones barely move.
        let f = up(need + 1).clamp(17, max_frames);
        parts.push(f);
        need = need.saturating_sub(f - 1);
    }
    parts
}

/// The frames the engine wrote into `dir` (named `prefix…png`), in order.
pub fn frames_in(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix)) && p.extension().is_some_and(|e| e == "png")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Average color and spread per channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub mean: [f32; 3],
    pub spread: [f32; 3],
}

pub fn stats_of(imgs: &[RgbaImage]) -> Stats {
    let (mut sum, mut sq, mut n) = ([0f64; 3], [0f64; 3], 0f64);
    for img in imgs {
        for p in img.pixels() {
            for c in 0..3 {
                let v = p[c] as f64;
                sum[c] += v;
                sq[c] += v * v;
            }
            n += 1.0;
        }
    }
    let n = n.max(1.0);
    let mean = sum.map(|s| s / n);
    let spread = [0, 1, 2].map(|c| (sq[c] / n - mean[c] * mean[c]).max(1.0).sqrt() as f32);
    Stats { mean: mean.map(|m| m as f32), spread }
}

/// A part's colors from a few of its frames, made small (quick).
fn stats(frames: &[PathBuf]) -> Stats {
    let step = (frames.len() / 6).max(1);
    let sample: Vec<RgbaImage> = frames.iter().step_by(step).filter_map(|p| image::open(p).ok()).map(|i| i.thumbnail(160, 160).to_rgba8()).collect();
    stats_of(&sample)
}

/// `img` with its colors moved from `from` toward `to` (`strength` 0-1).
pub fn match_colors(img: &mut RgbaImage, from: &Stats, to: &Stats, strength: f32) {
    let k = [0, 1, 2].map(|c| to.spread[c] / from.spread[c].max(1.0));
    for p in img.pixels_mut() {
        for c in 0..3 {
            let v = p[c] as f32;
            let moved = (v - from.mean[c]) * k[c] + to.mean[c];
            p[c] = (v + (moved - v) * strength).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// The parts as one MP4 of `total` frames: the first part whole, each later
/// one without the frame it started from, its colors matched to the first.
/// Returns the file's bytes and the first frame.
#[cfg(windows)]
pub fn join(parts: &[Vec<PathBuf>], total: usize, fps: u32, out: &Path, cancel: &std::sync::atomic::AtomicBool, progress: &dyn Fn(f64)) -> Result<(Vec<u8>, DynamicImage), String> {
    use std::sync::atomic::Ordering;
    let first_path = parts.first().and_then(|p| p.first()).ok_or("The video engine didn't produce any frames.")?;
    let first = image::open(first_path).map_err(|e| e.to_string())?;
    // Even sides, as the encoder needs.
    let (w, h) = (first.width() / 2 * 2, first.height() / 2 * 2);
    let base = stats(&parts[0]);
    let mut writer = super::mp4::Writer::create(out, w, h, fps)?;
    let mut written = 0usize;
    'parts: for (i, part) in parts.iter().enumerate() {
        let shift = (i > 0).then(|| stats(part));
        for path in part.iter().skip(usize::from(i > 0)) {
            if written >= total {
                break 'parts;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::download::CANCELLED.into());
            }
            let mut f = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
            if f.dimensions() != (w, h) {
                f = image::imageops::resize(&f, w, h, image::imageops::FilterType::Lanczos3);
            }
            if let Some(s) = &shift {
                match_colors(&mut f, s, &base, 0.7);
            }
            writer.frame(&f)?;
            written += 1;
            progress(written as f64 / total.max(1) as f64);
        }
    }
    writer.finish()?;
    Ok((std::fs::read(out).map_err(|e| e.to_string())?, first))
}

#[cfg(not(windows))]
pub fn join(_parts: &[Vec<PathBuf>], _total: usize, _fps: u32, _out: &Path, _cancel: &std::sync::atomic::AtomicBool, _progress: &dyn Fn(f64)) -> Result<(Vec<u8>, DynamicImage), String> {
    Err("Long clips are made on Windows only for now.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_cover_the_length_asked_for() {
        // Fits in one: one part, 4k+1 frames.
        assert_eq!(plan(73, 121), vec![73]);
        assert_eq!(plan(70, 121), vec![73]);
        // A minute at 24 fps (1441 frames) in 121-frame parts.
        let p = plan(1441, 121);
        assert!(p.iter().all(|f| (f - 1) % 4 == 0 && *f <= 121 && *f >= 17), "{p:?}");
        let made = p[0] + p[1..].iter().map(|f| f - 1).sum::<u32>();
        assert!(made >= 1441 && made < 1441 + 120, "{made}");
        assert_eq!(p.len(), 12);
        // Just over one part: a short second part.
        let p = plan(130, 121);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1], 17);
    }

    #[test]
    fn later_parts_take_the_first_parts_colors() {
        let warm = RgbaImage::from_pixel(4, 4, image::Rgba([200, 120, 80, 255]));
        let mut drifted = RgbaImage::from_pixel(4, 4, image::Rgba([170, 120, 110, 255]));
        let (to, from) = (stats_of(&[warm]), stats_of(std::slice::from_ref(&drifted)));
        match_colors(&mut drifted, &from, &to, 1.0);
        assert_eq!(drifted.get_pixel(0, 0).0, [200, 120, 80, 255]);
    }
}

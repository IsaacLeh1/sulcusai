// SPDX-License-Identifier: AGPL-3.0-only
//! FFmpeg, downloaded the first time a video is edited (a pinned build,
//! hash-checked): what a file holds, and the edits themselves (cut, join,
//! a sound track, a voice dubbed in), run like any engine for a job.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::{engine, AppState};

#[cfg(windows)]
const FFMPEG: &str = "ffmpeg.exe";
#[cfg(not(windows))]
const FFMPEG: &str = "ffmpeg";

/// FFmpeg and FFprobe, downloaded the first time.
pub async fn ensure(state: &Arc<AppState>, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<(PathBuf, PathBuf), String> {
    let spec = state.catalog.media.ffmpeg.as_ref().ok_or("Editing videos isn't available on this system yet.")?;
    let asset = spec.assets.get(engine::cpu_key()).ok_or("Editing videos isn't available on this system yet.")?;
    let dir = format!("ffmpeg-{}-{}", spec.build, engine::cpu_key());
    let getting = format!("Getting the video tools (once, {})", crate::size_label(asset.size));
    if engine::find_exe(&state.paths.engines.join(&dir), FFMPEG).is_none() {
        state.log("network", &format!("Downloading FFmpeg {} from {}", spec.build, crate::host_of(&asset.url)));
    }
    let ffmpeg = engine::ensure_unpacked(&state.paths, &dir, asset, FFMPEG, cancel, |r, t| progress(&getting, r as f64 / t.max(1) as f64)).await?;
    let probe = ffmpeg.with_file_name(FFMPEG.replace("ffmpeg", "ffprobe"));
    if !probe.is_file() {
        return Err("The video tools' download is missing FFprobe.".into());
    }
    Ok((ffmpeg, probe))
}

/// What a video or sound file holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio: bool,
    pub video: bool,
}

pub async fn probe(ffprobe: &Path, file: &Path) -> Result<Probe, String> {
    let mut cmd = tokio::process::Command::new(ffprobe);
    cmd.args(["-v", "error", "-show_entries", "stream=codec_type,width,height,r_frame_rate:format=duration", "-of", "json"]).arg(file).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(30), cmd.output()).await.map_err(|_| "FFprobe took too long.".to_string())?.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| "Couldn't read what's in that file.".to_string())?;
    let mut p = Probe { seconds: v.pointer("/format/duration").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0.0), ..Default::default() };
    for s in v["streams"].as_array().into_iter().flatten() {
        match s["codec_type"].as_str() {
            Some("video") if !p.video => {
                p.video = true;
                p.width = s["width"].as_u64().unwrap_or(0) as u32;
                p.height = s["height"].as_u64().unwrap_or(0) as u32;
                p.fps = s["r_frame_rate"].as_str().and_then(|r| r.split_once('/')).and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?.max(1.0))).unwrap_or(24.0);
            }
            Some("audio") => p.audio = true,
            _ => {}
        }
    }
    Ok(p)
}

/// Runs FFmpeg for a job: `total` seconds of output, for how far it is
/// (`progress` gets 0..1). Stop ends it.
pub async fn run(state: &AppState, ffmpeg: &Path, args: Vec<std::ffi::OsString>, total: f64, cancel: &AtomicBool, progress: &(dyn Fn(f64) + Sync)) -> Result<(), String> {
    let mut all: Vec<std::ffi::OsString> = ["-hide_banner", "-y", "-nostats", "-progress", "pipe:1"].map(Into::into).to_vec();
    all.extend(args);
    let cwd = state.paths.data.clone();
    super::proc::run(
        super::proc::Run { exe: ffmpeg, args: all, cwd: &cwd, log: &state.paths.logs.join("ffmpeg.log"), low_priority: state.limits().low_priority, job: state.job() },
        cancel,
        |line| {
            if let Some(us) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")).and_then(|v| v.trim().parse::<f64>().ok()) {
                if total > 0.0 {
                    progress((us / 1e6 / total).clamp(0.0, 1.0));
                }
            }
        },
    )
    .await
}

/// The video encoder's settings: H.264 at a quality that keeps detail, AAC sound.
pub fn mp4_out(out: &Path) -> Vec<std::ffi::OsString> {
    ["-c:v", "libx264", "-preset", "medium", "-crf", "19", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"].iter().map(Into::into).chain(std::iter::once(out.as_os_str().to_owned())).collect()
}

/// The filters that join clips of any size: each fitted into the first's
/// frame (letterboxed), at its frame rate, with silence for a clip that has
/// no sound.
pub fn join_filter(clips: &[Probe]) -> String {
    let first = clips.first().cloned().unwrap_or_default();
    let (w, h, fps) = ((first.width.max(2) / 2) * 2, (first.height.max(2) / 2) * 2, if first.fps > 0.0 { first.fps } else { 24.0 });
    let mut f = String::new();
    for (i, c) in clips.iter().enumerate() {
        f.push_str(&format!("[{i}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps:.3}[v{i}];"));
        if c.audio {
            f.push_str(&format!("[{i}:a]aresample=44100,aformat=channel_layouts=stereo[a{i}];"));
        } else {
            f.push_str(&format!("anullsrc=r=44100:cl=stereo,atrim=duration={:.3}[a{i}];", c.seconds.max(0.1)));
        }
    }
    for i in 0..clips.len() {
        f.push_str(&format!("[v{i}][a{i}]"));
    }
    f.push_str(&format!("concat=n={}:v=1:a=1[v][a]", clips.len()));
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_of_any_size_join_into_the_first_ones_frame() {
        let a = Probe { seconds: 4.0, width: 1280, height: 720, fps: 24.0, audio: true, video: true };
        let b = Probe { seconds: 3.0, width: 720, height: 1280, fps: 30.0, audio: false, video: true };
        let f = join_filter(&[a, b]);
        assert!(f.contains("[1:v]scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720"));
        assert!(f.contains("anullsrc=r=44100:cl=stereo,atrim=duration=3.000[a1]"), "silence for the clip without sound");
        assert!(f.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"));
    }
}

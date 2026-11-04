// SPDX-License-Identifier: AGPL-3.0-only
//! The creating-media options as they reach the engines: extra picture
//! arguments (tiles, a sketch or depth guide) and the video pre-pass (cut by
//! transcript, cleaned speech).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::regions::{self, Guide};
use crate::media::{imaging, store, RunCtx};
use crate::AppState;

/// A guide picture for a new one.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GuideSpec {
    /// A gallery id.
    #[serde(default)]
    pub image: Option<String>,
    /// Or a PNG as base64 (a sketch drawn in the window).
    #[serde(default)]
    pub png: Option<String>,
    pub kind: Guide,
    #[serde(default = "one")]
    pub strength: f32,
}

fn one() -> f32 {
    0.9
}

/// Picture options from phase 13, all off unless set.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct PictureExtra {
    pub tile: bool,
    pub guide: Option<GuideSpec>,
}

/// Where ControlNet files go (any .gguf or .safetensors with "depth",
/// "canny", "scribble" or "lineart" in its name).
pub fn control_dir(state: &AppState) -> PathBuf {
    state.paths.models.join("controlnet")
}

pub fn control_nets(state: &AppState) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(control_dir(state))
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".gguf") || n.ends_with(".safetensors")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn guide_bytes(ctx: &RunCtx<'_>, g: &GuideSpec) -> Result<Vec<u8>, String> {
    if let Some(id) = g.image.as_deref().filter(|i| !i.is_empty()) {
        let c = ctx.state.work_cipher()?;
        return store::read(&ctx.state.paths, &c, id);
    }
    let png = g.png.as_deref().ok_or("Draw or pick a guide picture first.")?;
    let b64 = png.split_once(',').map_or(png, |x| x.1);
    base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| "The guide picture couldn't be read.".to_string())
}

/// Adds the extra arguments for a picture of `w` x `h`.
pub(in crate::media) fn picture_args(ctx: &RunCtx<'_>, extra: &PictureExtra, w: u32, h: u32, args: &mut Vec<OsString>, work: &Path) -> Result<(), String> {
    if extra.tile {
        args.extend(super::tiles::tile_args());
    }
    if let Some(g) = &extra.guide {
        let nets = control_nets(ctx.state);
        let net = regions::pick_net(g.kind, &nets).ok_or_else(|| {
            format!("A {} guide needs a matching ControlNet file in {}.", match g.kind { Guide::Depth => "depth", _ => "sketch" }, control_dir(ctx.state).display())
        })?;
        let img = imaging::decode(&guide_bytes(ctx, g)?)?;
        let img = imaging::fill_to(&img, w, h);
        let p = work.join("guide.png");
        std::fs::write(&p, imaging::png(&img)?).map_err(|e| e.to_string())?;
        args.extend(regions::control_args(&control_dir(ctx.state).join(net).to_string_lossy(), &p.to_string_lossy(), g.kind, g.strength));
    }
    Ok(())
}

/// FFmpeg arguments for the pre-pass: the kept stretches joined, the speech
/// cleaned, or both. None when neither is asked.
pub fn prepass_args(input: &Path, out: &Path, keep: &[[f64; 2]], clean: Option<&str>, has_audio: bool) -> Option<Vec<OsString>> {
    if keep.is_empty() && clean.is_none() {
        return None;
    }
    let mut a: Vec<OsString> = vec!["-i".into(), input.as_os_str().to_owned()];
    let ranges: Vec<(f64, f64)> = keep.iter().filter(|[s, e]| e > s).map(|[s, e]| (*s, *e)).collect();
    if ranges.is_empty() {
        a.extend(["-map".into(), "0:v".into()]);
        if let (true, Some(f)) = (has_audio, clean) {
            a.extend(["-map".into(), "0:a".into(), "-af".into(), f.into()]);
        }
    } else {
        let mut f = super::textcut::filter(&ranges, has_audio);
        let audio = if has_audio {
            match clean {
                Some(c) => {
                    f = f.replace("[outa]", "[cut_a]");
                    f.push_str(&format!(";[cut_a]{c}[outa]"));
                    true
                }
                None => true,
            }
        } else {
            false
        };
        a.extend(["-filter_complex".into(), f.into(), "-map".into(), "[outv]".into()]);
        if audio {
            a.extend(["-map".into(), "[outa]".into()]);
        }
    }
    a.extend(crate::media::ffmpeg::mp4_out(out));
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(a: &[OsString]) -> String {
        a.iter().map(|x| x.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    }
    #[test]
    fn prepass_shapes() {
        let (i, o) = (Path::new("in.mp4"), Path::new("out.mp4"));
        assert!(prepass_args(i, o, &[], None, true).is_none());
        let cut = s(&prepass_args(i, o, &[[0.0, 1.0], [2.0, 3.0]], None, true).unwrap());
        assert!(cut.contains("concat=n=2:v=1:a=1[outv][outa]") && cut.contains("-map [outa]"), "{cut}");
        let both = s(&prepass_args(i, o, &[[0.0, 1.0]], Some("highpass=f=80"), true).unwrap());
        assert!(both.contains("[cut_a];[cut_a]highpass=f=80[outa]"), "{both}");
        let clean = s(&prepass_args(i, o, &[], Some("highpass=f=80"), true).unwrap());
        assert!(clean.contains("-af highpass=f=80"));
        let silent = s(&prepass_args(i, o, &[[0.0, 1.0]], Some("x"), false).unwrap());
        assert!(!silent.contains("[outa]"));
    }
    #[test]
    fn extra_defaults_off() {
        let e: PictureExtra = serde_json::from_str("{}").unwrap();
        assert!(!e.tile && e.guide.is_none());
    }
}

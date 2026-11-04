// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures from regions, sketches and depth, and live painting.
//! sd.cpp has no regional attention, so regions are drawn as a first pass of
//! the whole scene and then one masked pass (inpaint) per region; sketches and
//! depth go in through a ControlNet.

use std::ffi::OsString;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    /// Fractions of the picture, 0..1.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub prompt: String,
}

/// Where a region sits, in words the first pass understands.
fn place(r: &Region) -> &'static str {
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    match ((cx * 3.0).floor().clamp(0.0, 2.0) as u8, (cy * 3.0).floor().clamp(0.0, 2.0) as u8) {
        (0, 0) => "top left",
        (1, 0) => "top",
        (2, 0) => "top right",
        (0, 1) => "left",
        (1, 1) => "center",
        (2, 1) => "right",
        (0, _) => "bottom left",
        (1, _) => "bottom",
        _ => "bottom right",
    }
}

/// The first pass: the whole scene with each region named where it goes.
pub fn scene_prompt(base: &str, regions: &[Region]) -> String {
    let mut parts = vec![base.trim().to_string()];
    parts.extend(regions.iter().filter(|r| !r.prompt.trim().is_empty()).map(|r| format!("{} at the {}", r.prompt.trim(), place(r))));
    parts.retain(|p| !p.is_empty());
    parts.join(", ")
}

/// A region's mask in pixels: white inside, black outside, as RGB bytes.
pub fn mask(r: &Region, width: u32, height: u32) -> Vec<u8> {
    let x0 = (r.x.clamp(0.0, 1.0) * width as f64).round() as u32;
    let y0 = (r.y.clamp(0.0, 1.0) * height as f64).round() as u32;
    let x1 = ((r.x + r.w).clamp(0.0, 1.0) * width as f64).round() as u32;
    let y1 = ((r.y + r.h).clamp(0.0, 1.0) * height as f64).round() as u32;
    let mut out = vec![0u8; (width * height * 3) as usize];
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * width + x) * 3) as usize;
            out[i..i + 3].copy_from_slice(&[255, 255, 255]);
        }
    }
    out
}

pub fn check(regions: &[Region]) -> Result<(), String> {
    if regions.len() > 8 {
        return Err("At most 8 regions in one picture.".into());
    }
    for r in regions {
        if r.w <= 0.02 || r.h <= 0.02 || r.x < 0.0 || r.y < 0.0 || r.x + r.w > 1.001 || r.y + r.h > 1.001 {
            return Err("A region is too small or outside the picture.".into());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Guide {
    /// A line drawing, used as is.
    Sketch,
    /// A photo turned into edges first (sd.cpp's --canny).
    Edges,
    /// A depth map (near is white).
    Depth,
}

/// ControlNet arguments for a guide picture.
pub fn control_args(net: &str, image: &str, guide: Guide, strength: f32) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["--control-net".into(), net.into(), "--control-image".into(), image.into()];
    a.extend(["--control-strength".into(), format!("{:.2}", strength.clamp(0.0, 2.0)).into()]);
    if guide == Guide::Edges {
        a.push("--canny".into());
    }
    a
}

/// The ControlNet file that matches a guide, from the installed ones' names.
pub fn pick_net<'a>(guide: Guide, installed: &'a [String]) -> Option<&'a String> {
    let want: &[&str] = match guide {
        Guide::Sketch => &["scribble", "lineart", "sketch", "canny"],
        Guide::Edges => &["canny", "lineart"],
        Guide::Depth => &["depth"],
    };
    want.iter().find_map(|w| installed.iter().find(|n| n.to_lowercase().contains(w)))
}

/// Live painting: a quick, small picture after each pause in drawing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Live {
    pub steps: u32,
    pub width: u32,
    pub height: u32,
    pub wait_ms: u64,
    /// How much of the drawing is kept (img2img strength is 1 - this).
    pub keep: f32,
}

/// Turbo/lightning models draw in 1 to 4 steps; others need more and wait longer.
pub fn live(model_id: &str, aspect: f64) -> Live {
    let fast = ["turbo", "lightning", "lcm", "hyper", "schnell"].iter().any(|k| model_id.to_lowercase().contains(k));
    let side = 512.0;
    let (w, h) = if aspect >= 1.0 { (side, side / aspect) } else { (side * aspect, side) };
    let r8 = |v: f64| ((v / 8.0).round() as u32 * 8).max(64);
    Live { steps: if fast { 4 } else { 12 }, width: r8(w), height: r8(h), wait_ms: if fast { 300 } else { 900 }, keep: 0.45 }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(x: f64, y: f64, w: f64, h: f64, p: &str) -> Region {
        Region { x, y, w, h, prompt: p.into() }
    }
    #[test]
    fn scene_names_places() {
        let s = scene_prompt("a park", &[r(0.0, 0.0, 0.3, 0.3, "a red kite"), r(0.6, 0.6, 0.4, 0.4, "a dog")]);
        assert_eq!(s, "a park, a red kite at the top left, a dog at the bottom right");
    }
    #[test]
    fn mask_marks_inside_only() {
        let m = mask(&r(0.5, 0.5, 0.5, 0.5, "x"), 4, 4);
        assert_eq!(m[0], 0);
        assert_eq!(m[((3 * 4 + 3) * 3) as usize], 255);
        assert_eq!(m.iter().filter(|&&b| b == 255).count(), 4 * 3);
    }
    #[test]
    fn checks_regions() {
        assert!(check(&[r(0.0, 0.0, 0.5, 0.5, "a")]).is_ok());
        assert!(check(&[r(0.8, 0.0, 0.5, 0.5, "a")]).is_err());
    }
    #[test]
    fn control() {
        let a: Vec<String> = control_args("cn.gguf", "s.png", Guide::Edges, 0.8).iter().map(|s| s.to_string_lossy().into()).collect();
        assert_eq!(a.join(" "), "--control-net cn.gguf --control-image s.png --control-strength 0.80 --canny");
        let nets = vec!["control_v11f1p_sd15_depth.safetensors".to_string(), "control_v11p_sd15_scribble.safetensors".to_string()];
        assert!(pick_net(Guide::Depth, &nets).unwrap().contains("depth"));
        assert!(pick_net(Guide::Sketch, &nets).unwrap().contains("scribble"));
        assert!(pick_net(Guide::Edges, &nets).is_none());
    }
    #[test]
    fn live_settings() {
        let l = live("sdxl-turbo", 16.0 / 9.0);
        assert_eq!((l.steps, l.width, l.height % 8), (4, 512, 0));
        assert_eq!(live("sd15", 1.0).steps, 12);
    }
}

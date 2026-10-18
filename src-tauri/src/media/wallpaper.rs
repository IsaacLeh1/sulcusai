// SPDX-License-Identifier: AGPL-3.0-only
//! Desktop wallpapers: a picture or a looping clip made for a screen's exact
//! size. The model makes it at its own size in the screen's shape; it's then
//! enlarged (with the upscaler when one is installed) and cut to the screen.
//! OLED-friendly ones are asked for on true black, and their near-blacks are
//! made pure black afterwards so those pixels stay off. A moving one loops
//! without a jump: its last frames fade into its first.

use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub oled: bool,
}

impl Spec {
    pub fn checked(&self) -> Result<(u32, u32), String> {
        let (w, h) = (self.width, self.height);
        if !(320..=16384).contains(&w) || !(320..=16384).contains(&h) {
            return Err("Pick a screen size between 320 and 16384 pixels.".into());
        }
        Ok((w, h))
    }
}

/// The widest shape models make well; wider screens get the middle of it.
const MAX_RATIO: f64 = 2.4;

/// The size to make it at: the screen's shape (up to 2.4:1) at about
/// `side`² pixels, in steps of `align`.
pub fn make_size(spec: &Spec, side: u32, align: u32) -> (u32, u32) {
    let ratio = (spec.width as f64 / spec.height.max(1) as f64).clamp(1.0 / MAX_RATIO, MAX_RATIO);
    let area = side as f64 * side as f64;
    let w = (area * ratio).sqrt();
    let h = area / w;
    let a = align.max(16) as f64;
    let step = |v: f64| ((v / a).round().max(1.0) * a) as u32;
    (step(w), step(h))
}

/// What the description must serve, for the prompt writer.
pub fn notes(spec: &Spec, moving: bool) -> String {
    // Naming the desktop, icons or a taskbar made models draw them (a fake
    // taskbar across the bottom): the picture is described as art only.
    let mut n = format!(
        "This is a {} piece of wide background art, {}×{} pixels. Compose it for that shape: one clear focal area near the middle or slightly right, the left edge and the bottom strip simple and quiet, nothing important cut at the edges. It is only the scene itself: no screens, windows, icons, menus, bars, frames, borders, text or logos unless asked.",
        if moving { "moving (looping)" } else { "still" },
        spec.width,
        spec.height
    );
    if moving {
        n.push_str(" The motion is slow and continuous (drifting clouds, flowing water, gentle light changes), the camera nearly still, so it loops calmly.");
    }
    if spec.oled {
        n.push_str(" It's for an OLED screen: a true pure black background covering most of the picture, the subject lit against the darkness, no large bright or gray areas, no white skies.");
    }
    n
}

/// Words added to a description the prompt writer didn't write.
pub fn plain_words(spec: &Spec) -> &'static str {
    if spec.oled {
        ", wide background art, on a pure black background, deep true-black shadows, high contrast, subject glowing against darkness, no text, no interface"
    } else {
        ", wide background art, balanced composition, clean uncluttered edges, no text, no interface"
    }
}

/// Below this brightness a pixel goes pure black; between it and `KNEE` it
/// fades down smoothly, so there's no visible edge.
/// (At 14 and 42, a dim aurora's glow was mostly blacked out.)
const BLACK: f32 = 8.0;
const KNEE: f32 = 24.0;

/// Near-blacks to pure black (OLED pixels then stay off).
pub fn crush_blacks(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let l = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        if l >= KNEE {
            continue;
        }
        let k = if l <= BLACK {
            0.0
        } else {
            let t = (l - BLACK) / (KNEE - BLACK);
            t * t * (3.0 - 2.0 * t)
        };
        for c in 0..3 {
            p[c] = (p[c] as f32 * k).round() as u8;
        }
    }
}

/// The share of pixels that are pure black (shown with OLED wallpapers).
pub fn black_share(img: &RgbaImage) -> f32 {
    let n = img.pixels().filter(|p| p[0] == 0 && p[1] == 0 && p[2] == 0).count();
    n as f32 / (img.width() * img.height()).max(1) as f32
}

/// The finished still: cut to the screen, OLED blacks applied.
pub fn finish(img: &DynamicImage, spec: &Spec) -> DynamicImage {
    let mut out = super::imaging::fill_to(img, spec.width, spec.height).to_rgba8();
    if spec.oled {
        crush_blacks(&mut out);
    }
    DynamicImage::ImageRgba8(out)
}

/// The order a loop plays its frames in: the middle as made, then the last
/// `fade` frames each blended toward one of the first, so the last shown is
/// almost the first's neighbor and the loop restarts without a jump.
/// Each entry: the frame, and the frame it's blended toward with its weight.
pub fn loop_plan(n: usize) -> Vec<(usize, Option<(usize, f32)>)> {
    if n < 8 {
        return (0..n).map(|i| (i, None)).collect();
    }
    let fade = (n / 4).min(16);
    let mut out: Vec<(usize, Option<(usize, f32)>)> = (fade..n - fade).map(|i| (i, None)).collect();
    for k in 0..fade {
        out.push((n - fade + k, Some((k, (k + 1) as f32 / (fade + 1) as f32))));
    }
    out
}

pub fn blend(a: &RgbaImage, b: &RgbaImage, t: f32) -> RgbaImage {
    RgbaImage::from_fn(a.width(), a.height(), |x, y| {
        let (p, q) = (a.get_pixel(x, y), b.get_pixel(x, y));
        image::Rgba([0, 1, 2, 3].map(|i| (p[i] as f32 * (1.0 - t) + q[i] as f32 * t).round() as u8))
    })
}

/// The size a clip is encoded at: the screen's, within what H.264 encoders
/// take (4096 wide, 2304 tall), in even numbers. Wallpaper apps stretch
/// the rest of the way.
pub fn clip_size(spec: &Spec) -> (u32, u32) {
    let k = (4096.0 / spec.width as f64).min(2304.0 / spec.height as f64).min(1.0);
    let even = |v: f64| ((v / 2.0).floor() as u32 * 2).max(2);
    (even(spec.width as f64 * k), even(spec.height as f64 * k))
}

/// Frames the engine wrote (`frame_000.png`…), in order.
pub fn frame_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("frame_")) && p.extension().is_some_and(|e| e == "png")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The looping MP4 from the engine's frames: each frame enlarged and cut to
/// the screen, blacks crushed for OLED, the end blended into the start.
/// Returns the file's bytes and the first frame (its poster).
#[cfg(windows)]
pub fn encode_loop(frames: &[PathBuf], spec: &Spec, fps: u32, out: &Path, cancel: &std::sync::atomic::AtomicBool, progress: &dyn Fn(f64)) -> Result<(Vec<u8>, DynamicImage), String> {
    use std::sync::atomic::Ordering;
    if frames.is_empty() {
        return Err("The video engine didn't produce any frames.".into());
    }
    let (w, h) = clip_size(spec);
    // Frames made at about 1000 pixels and enlarged to a big screen get a
    // light sharpening (the upscaler, run on every frame, took 8 s a frame).
    let enlarged = image::image_dimensions(&frames[0]).map_or(1.0, |(fw, fh)| (w as f64 / fw.max(1) as f64).max(h as f64 / fh.max(1) as f64));
    let load = |i: usize| -> Result<RgbaImage, String> {
        let img = super::imaging::decode(&std::fs::read(&frames[i]).map_err(|e| e.to_string())?)?;
        let fitted = super::imaging::fill_to(&img, w, h);
        let mut f = if enlarged >= 1.6 { image::imageops::unsharpen(&fitted.to_rgba8(), 1.2, 3) } else { fitted.to_rgba8() };
        if spec.oled {
            crush_blacks(&mut f);
        }
        Ok(f)
    };
    let plan = loop_plan(frames.len());
    let mut writer = super::mp4::Writer::create(out, w, h, fps)?;
    let mut poster = None;
    for (n, (i, toward)) in plan.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::download::CANCELLED.into());
        }
        let mut f = load(*i)?;
        if let Some((j, t)) = toward {
            f = blend(&f, &load(*j)?, *t);
        }
        writer.frame(&f)?;
        if poster.is_none() {
            poster = Some(DynamicImage::ImageRgba8(f));
        }
        progress((n + 1) as f64 / plan.len() as f64);
    }
    writer.finish()?;
    let bytes = std::fs::read(out).map_err(|e| e.to_string())?;
    Ok((bytes, poster.unwrap_or_else(|| DynamicImage::new_rgb8(w, h))))
}

#[cfg(not(windows))]
pub fn encode_loop(_frames: &[PathBuf], _spec: &Spec, _fps: u32, _out: &Path, _cancel: &std::sync::atomic::AtomicBool, _progress: &dyn Fn(f64)) -> Result<(Vec<u8>, DynamicImage), String> {
    Err("Moving wallpapers are made on Windows only for now.".into())
}

// ---------- setting it ----------

/// Where wallpapers are copied to so Windows (or Lively) can show them:
/// Pictures\SulcusAI Wallpapers.
pub fn folder() -> Result<PathBuf, String> {
    let pictures = pictures_dir().ok_or("Couldn't find your Pictures folder.")?;
    let dir = pictures.join("SulcusAI Wallpapers");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// The Pictures folder (wherever it's been moved, OneDrive included).
#[cfg(windows)]
fn pictures_dir() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

#[cfg(not(windows))]
fn pictures_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures"))
}

/// Sets a picture as the desktop wallpaper: on the screen whose top-left
/// corner is `at` (physical pixels), or on every screen.
#[cfg(windows)]
pub fn set_still(path: &Path, at: Option<(i32, i32)>) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper, DWPOS_FILL};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dw: IDesktopWallpaper = CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).map_err(|e| format!("Windows didn't let the wallpaper be set: {e}"))?;
        let file = HSTRING::from(path.as_os_str());
        let mut monitor: Option<HSTRING> = None;
        if let Some((x, y)) = at {
            let n = dw.GetMonitorDevicePathCount().unwrap_or(0);
            for i in 0..n {
                let Ok(id) = dw.GetMonitorDevicePathAt(i) else { continue };
                let hs = HSTRING::from_wide(id.as_wide()).unwrap_or_default();
                windows::Win32::System::Com::CoTaskMemFree(Some(id.0 as *const _));
                if let Ok(r) = dw.GetMonitorRECT(&hs) {
                    if (r.left - x).abs() <= 2 && (r.top - y).abs() <= 2 {
                        monitor = Some(hs);
                        break;
                    }
                }
            }
        }
        dw.SetPosition(DWPOS_FILL).map_err(|e| e.to_string())?;
        match &monitor {
            Some(id) => dw.SetWallpaper(id, &file),
            None => dw.SetWallpaper(PCWSTR::null(), &file),
        }
        .map_err(|e| format!("Windows didn't take the wallpaper: {e}"))
    }
}

#[cfg(not(windows))]
pub fn set_still(_path: &Path, _at: Option<(i32, i32)>) -> Result<(), String> {
    Err("Setting the wallpaper from here works on Windows. The picture is saved in your Pictures folder.".into())
}

/// Lively Wallpaper, when it's installed: it plays videos as wallpapers
/// (Windows can't by itself) and takes `setwp --file` through its main
/// program or its `livelycu` tool.
pub fn lively() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let mut places = vec![local.join("Programs").join("Lively Wallpaper").join("Lively.exe"), local.join("Programs").join("Lively Wallpaper").join("livelycu.exe")];
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        places.push(PathBuf::from(pf).join("Lively Wallpaper").join("Lively.exe"));
    }
    places.push(local.join("Microsoft").join("WindowsApps").join("livelycu.exe"));
    places.into_iter().find(|p| p.exists())
}

/// Hands a video to Lively Wallpaper.
pub fn set_moving(path: &Path) -> Result<(), String> {
    let exe = lively().ok_or("NO_LIVELY")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["setwp", "--file"]).arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("Lively Wallpaper didn't start: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Lively Wallpaper couldn't set it: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_in_the_screens_shape() {
        let s = Spec { width: 3840, height: 2160, oled: false };
        let (w, h) = make_size(&s, 1024, 16);
        assert!(w % 16 == 0 && h % 16 == 0);
        assert!(((w as f64 / h as f64) - 16.0 / 9.0).abs() < 0.05, "{w}x{h}");
        // Super-wide screens: made at 2.4:1, the middle shown.
        let (w, h) = make_size(&Spec { width: 5120, height: 1440, oled: false }, 1024, 16);
        assert!((w as f64 / h as f64) <= 2.45, "{w}x{h}");
        // A tall (rotated) screen.
        let (w, h) = make_size(&Spec { width: 1440, height: 2560, oled: false }, 1024, 32);
        assert!(h > w && w % 32 == 0);
        assert!(Spec { width: 100, height: 100, oled: false }.checked().is_err());
    }

    #[test]
    fn near_blacks_turn_off_and_the_rest_stays() {
        let mut img = RgbaImage::from_fn(4, 1, |x, _| image::Rgba(match x {
            0 => [4, 6, 5, 255],
            1 => [16, 16, 16, 255],
            2 => [120, 60, 200, 255],
            _ => [0, 0, 0, 255],
        }));
        crush_blacks(&mut img);
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 255]);
        let mid = img.get_pixel(1, 0)[0];
        assert!(mid > 0 && mid < 16, "a smooth fade, not a cliff: {mid}");
        let mut glow = RgbaImage::from_pixel(1, 1, image::Rgba([20, 60, 50, 255]));
        crush_blacks(&mut glow);
        assert_eq!(glow.get_pixel(0, 0).0, [20, 60, 50, 255], "a dim glow stays");
        assert_eq!(img.get_pixel(2, 0).0, [120, 60, 200, 255], "bright colors untouched");
        assert!((black_share(&img) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_loop_ends_where_it_starts() {
        let plan = loop_plan(121);
        let fade = 16;
        assert_eq!(plan.len(), 121 - fade);
        assert_eq!(plan[0], (fade, None), "starts after the frames the end fades into");
        let (last, toward) = *plan.last().unwrap();
        assert_eq!(last, 120);
        let (j, t) = toward.unwrap();
        assert_eq!(j, fade - 1, "the last shown is mostly the frame before the first");
        assert!(t > 0.9);
        assert_eq!(loop_plan(5).len(), 5);
    }

    #[test]
    fn clips_fit_the_encoder() {
        assert_eq!(clip_size(&Spec { width: 3840, height: 2160, oled: false }), (3840, 2160));
        let (w, h) = clip_size(&Spec { width: 5120, height: 1440, oled: false });
        assert!(w <= 4096 && h <= 2304 && w % 2 == 0 && h % 2 == 0);
        let (w, h) = clip_size(&Spec { width: 7680, height: 4320, oled: false });
        assert!(w <= 4096 && h <= 2304, "{w}x{h}");
    }
}

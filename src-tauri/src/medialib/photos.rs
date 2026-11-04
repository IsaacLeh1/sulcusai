// SPDX-License-Identifier: AGPL-3.0-only
//! The Photos library (pictures in the folders the person picks, newest
//! first, by month, with small previews) and the Screenshot tool.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const KINDS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];
const MAX_PHOTOS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Photo {
    pub path: String,
    pub name: String,
    /// Seconds since 1970, from when the file was last changed.
    pub taken: i64,
    pub size: u64,
}

pub fn is_picture(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| KINDS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Pictures under `folders`, newest first. Hidden folders and the ones
/// programs keep caches in are skipped.
pub fn scan(folders: &[PathBuf]) -> Vec<Photo> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = folders.to_vec();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                if !name.starts_with('.') && !matches!(name.to_lowercase().as_str(), "node_modules" | "cache" | "thumbnails" | "$recycle.bin") {
                    stack.push(p);
                }
            } else if is_picture(&p) && out.len() < MAX_PHOTOS {
                let taken = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                out.push(Photo { path: p.to_string_lossy().to_string(), name, taken, size: meta.len() });
            }
        }
    }
    out.sort_by(|a, b| b.taken.cmp(&a.taken).then(a.path.cmp(&b.path)));
    out
}

/// "2026-11" for grouping by month.
pub fn month(taken: i64) -> String {
    chrono::DateTime::from_timestamp(taken, 0).map(|d| d.format("%Y-%m").to_string()).unwrap_or_default()
}

/// A small JPEG preview, longest side `px`.
pub fn thumb(path: &Path, px: u32) -> Result<Vec<u8>, String> {
    if !is_picture(path) {
        return Err("That isn't a picture.".into());
    }
    let img = image::open(path).map_err(|e| format!("Couldn't open the picture: {e}"))?;
    let small = img.thumbnail(px, px).to_rgb8();
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 80).encode_image(&small).map_err(|e| e.to_string())?;
    Ok(jpg)
}

/// Where screenshots go: Pictures\Screenshots, as Windows keeps them.
pub fn screenshot_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Pictures").join("Screenshots")
}

pub fn screenshot_name(now: chrono::DateTime<chrono::Local>) -> String {
    format!("Screenshot {}.png", now.format("%Y-%m-%d %H%M%S"))
}

/// The monitor the mouse is on, saved as a PNG; the file's path back.
pub fn screenshot() -> Result<PathBuf, String> {
    let img = crate::media::screen::capture()?;
    let dir = screenshot_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(screenshot_name(chrono::Local::now()));
    img.save(&path).map_err(|e| format!("Couldn't save the screenshot: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_pictures_newest_first() {
        let dir = std::env::temp_dir().join(format!("sulcus-photos-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("trip")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        image::RgbImage::new(40, 20).save(dir.join("trip").join("a.png")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        image::RgbImage::new(10, 10).save(dir.join("b.JPG")).unwrap();
        std::fs::write(dir.join(".hidden").join("c.png"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let found = scan(&[dir.clone()]);
        let names: Vec<_> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b.JPG", "a.png"]);
        let t = thumb(Path::new(&found[1].path), 16).unwrap();
        let back = image::load_from_memory(&t).unwrap();
        assert_eq!((back.width(), back.height()), (16, 8));
        assert!(thumb(&dir.join("notes.txt"), 16).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn names_and_months() {
        assert_eq!(month(1_793_750_400), "2026-11");
        let t = chrono::Local.with_ymd_and_hms(2026, 11, 4, 9, 5, 7).unwrap();
        assert_eq!(screenshot_name(t), "Screenshot 2026-11-04 090507.png");
    }
    use chrono::TimeZone;
}

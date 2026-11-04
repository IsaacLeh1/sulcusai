// SPDX-License-Identifier: AGPL-3.0-only
//! Scan: a photo of a page (taken on the phone or picked on the PC) kept in
//! the Scans folder, its words read by the vision model, saved as a scan
//! record and made findable through the Find index.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use crate::{AppState, AppStateRef};

const READ_ASK: &str = "Copy out all the text on this page exactly, keeping its line breaks. Then on a last line starting with \"Title:\" give a short title for the document. Write nothing else.";

pub fn scans_dir(data: &Path) -> PathBuf {
    data.join("Scans")
}

/// The text and a title from the model's reply.
pub fn split_title(reply: &str) -> (String, String) {
    let reply = reply.trim();
    match reply.rfind("Title:") {
        Some(i) => (reply[..i].trim().to_string(), reply[i + 6..].trim().trim_matches('"').to_string()),
        None => (reply.to_string(), reply.lines().next().unwrap_or("").chars().take(60).collect()),
    }
}

/// A name for the stored copy that never replaces another scan.
pub fn stored_name(dir: &Path, ext: &str) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S");
    let mut p = dir.join(format!("Scan {stamp}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("Scan {stamp} ({n}).{ext}"));
        n += 1;
    }
    p
}

async fn read_words(state: &Arc<AppState>, path: &Path) -> Result<String, String> {
    let img = image::open(path).map_err(|e| format!("Couldn't open the picture: {e}"))?;
    crate::vision::read(state, &[img], READ_ASK, None, 2000).await
}

/// Keeps a scan: copies the picture into the Scans folder, reads its words
/// when a vision model is installed, and saves a scan record.
#[tauri::command]
pub async fn scan_save(state: AppStateRef<'_>, path: String) -> Result<super::records::Record, String> {
    let state: Arc<AppState> = state.inner().clone();
    let src = PathBuf::from(&path);
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return Err("A scan is a photo (JPEG, PNG or WebP).".into());
    }
    let dir = scans_dir(&state.paths.data);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = stored_name(&dir, &ext);
    std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
    let (text, title, note) = match read_words(&state, &dest).await {
        Ok(reply) => {
            let (t, title) = split_title(&reply);
            (t, title, None)
        }
        Err(e) => (String::new(), String::new(), Some(e)),
    };
    let data = json!({ "title": title, "file": dest.display().to_string(), "text": text, "unread": note });
    let rec = super::with_records(&state, |r, c| r.put(c, "scan", None, &data))?;
    let words = format!("{title}\n{text}");
    let d = dest.display().to_string();
    let _ = super::with_find(&state.paths.data, |i| {
        if !i.roots().iter().any(|r| Path::new(r) == dir.canonicalize().unwrap_or(dir.clone())) {
            let _ = i.add_root(&dir);
        } else {
            let _ = i.refresh_root(&dir.canonicalize().unwrap_or(dir.clone()).display().to_string());
        }
        let canon = Path::new(&d).canonicalize().map(|p| p.display().to_string()).unwrap_or(d.clone());
        let _ = i.set_caption(&canon, &words);
    });
    Ok(rec)
}

/// Describes up to `limit` indexed pictures that have no description yet,
/// so they can be found by what they show. Runs only when asked.
#[tauri::command]
pub async fn find_describe_pictures(state: AppStateRef<'_>, limit: Option<usize>) -> Result<usize, String> {
    let state: Arc<AppState> = state.inner().clone();
    let todo = super::with_find(&state.paths.data, |i| i.uncaptioned_pictures(limit.unwrap_or(20).min(200)))?;
    let mut done = 0;
    for p in todo {
        let Ok(img) = image::open(&p) else { continue };
        let said = crate::vision::read(&state, &[img], "Describe this picture in one or two sentences: the main things, people, place and any visible text.", None, 200).await?;
        if super::with_find(&state.paths.data, |i| i.set_caption(&p, &said))?.unwrap_or(false) {
            done += 1;
        }
    }
    Ok(done)
}

/// The JPEG photo on each asked page of a scanned PDF (scanners store each
/// page as one JPEG, which can be read without drawing the page).
pub fn page_photos(path: &Path, pages: &[usize]) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let doc = lopdf::Document::load(path).map_err(|e| format!("Couldn't read this PDF: {e}"))?;
    let mut out = vec![];
    for (n, id) in doc.get_pages() {
        let n = n as usize;
        if !pages.contains(&n) {
            continue;
        }
        let imgs = doc.get_page_images(id).unwrap_or_default();
        let best = imgs
            .iter()
            .filter(|i| i.filters.as_ref().is_some_and(|f| f.iter().any(|x| x == "DCTDecode")))
            .max_by_key(|i| i.width * i.height);
        if let Some(i) = best {
            out.push((n, i.content.to_vec()));
        }
    }
    Ok(out)
}

/// Makes a scanned PDF searchable: the photo on each page with no text is
/// read by the vision model; the words go into the Find index for that PDF
/// and come back marked with their pages.
#[tauri::command]
pub async fn pdf_read_scans(state: AppStateRef<'_>, path: String) -> Result<String, String> {
    let state: Arc<AppState> = state.inner().clone();
    let src = PathBuf::from(&path);
    let pages = crate::docs::pdf_pages(&src)?;
    let scanned = super::pdf::scanned_pages(&pages);
    if scanned.is_empty() {
        return Err("Every page of this PDF already has text.".into());
    }
    let photos = page_photos(&src, &scanned)?;
    if photos.is_empty() {
        return Err("This PDF's pages aren't stored as photos SulcusAI can read.".into());
    }
    let mut text = String::new();
    for (n, bytes) in photos {
        let Ok(img) = image::load_from_memory(&bytes) else { continue };
        let said = crate::vision::read(&state, &[img], "Copy out all the text on this page exactly, keeping its line breaks. Write nothing else.", None, 2000).await?;
        text.push_str(&format!("[page {n}]\n{}\n\n", said.trim()));
    }
    let canon = src.canonicalize().map(|p| p.display().to_string()).unwrap_or(path);
    let _ = super::with_find(&state.paths.data, |i| i.set_caption(&canon, &text));
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_comes_off_the_end() {
        assert_eq!(split_title("Rent receipt\n$900 paid\nTitle: \"March rent\""), ("Rent receipt\n$900 paid".into(), "March rent".into()));
        assert_eq!(split_title("just text").1, "just text");
    }

    #[test]
    fn stored_names_never_clash() {
        let d = std::env::temp_dir().join(format!("sulcusai-scan-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        let a = stored_name(&d, "jpg");
        std::fs::write(&a, b"x").unwrap();
        let b = stored_name(&d, "jpg");
        assert_ne!(a, b);
        std::fs::remove_dir_all(&d).ok();
    }
}

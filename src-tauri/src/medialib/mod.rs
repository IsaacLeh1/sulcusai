// SPDX-License-Identifier: AGPL-3.0-only
//! The media library and reading: the parts that don't need a model or the
//! network, so they can be tested on their own.
//!
//! - `index`: transcripts with speakers, cut into chapters, searched by words.
//! - `subs`: SRT and WebVTT read for the player and written as captions.
//! - `feeds`: RSS and Atom read for podcasts and the news in the brief.
//! - `ebook`: EPUB books read chapter by chapter.
//! - `timeline`: the screen-history switch, off until turned on, behind a PIN.

pub mod ebook;
pub mod feeds;
pub mod index;
pub mod live;
pub mod photos;
pub mod store;
pub mod subs;
pub mod timeline;

use crate::AppStateRef;

#[tauri::command(async)]
pub fn media_subtitles(path: String) -> Result<Vec<subs::Cue>, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(subs::parse(&text))
}

#[tauri::command(async)]
pub fn media_feed_parse(xml: String) -> feeds::Feed {
    feeds::parse(&xml)
}

#[tauri::command(async)]
pub fn media_ebook_open(path: String) -> Result<ebook::Book, String> {
    ebook::open(std::path::Path::new(&path))
}

#[tauri::command(async)]
pub fn media_timeline_view(state: AppStateRef) -> timeline::View {
    let conn = state.db.lock().unwrap();
    timeline::view(&conn)
}

#[tauri::command(async)]
pub fn media_timeline_set(state: AppStateRef, settings: timeline::Settings, pin: Option<String>) -> Result<(), String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    let conn = state.db.lock().unwrap();
    timeline::set(&conn, settings, pin.as_deref())
}

#[tauri::command(async)]
pub fn media_timeline_set_pin(state: AppStateRef, pin: Option<String>, new_pin: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    timeline::set_pin(&conn, pin.as_deref(), &new_pin)
}

// ---------- the Media index ----------

#[tauri::command]
pub async fn media_index_add(state: AppStateRef<'_>, path: String) -> Result<store::Summary, String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    let st = state.inner().clone();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    store::add(&st, &path, &cancel).await.map(|e| store::summary(&e))
}

#[tauri::command(async)]
pub fn media_index_list(state: AppStateRef) -> Result<Vec<store::Summary>, String> {
    let c = state.work_cipher()?;
    Ok(store::all(&state.db.lock().unwrap(), &c).iter().map(store::summary).collect())
}

#[tauri::command(async)]
pub fn media_index_get(state: AppStateRef, id: String) -> Result<store::Entry, String> {
    let c = state.work_cipher()?;
    store::get(&state.db.lock().unwrap(), &c, &id).ok_or_else(|| "That file is no longer in the Media index.".into())
}

#[tauri::command(async)]
pub fn media_index_remove(state: AppStateRef, id: String) -> Result<(), String> {
    let path = state.work_cipher().ok().and_then(|c| store::get(&state.db.lock().unwrap(), &c, &id)).map(|e| e.path);
    store::remove(&state.db.lock().unwrap(), &id)?;
    if let Some(p) = path {
        crate::files::media_transcript_removed(&state.paths.data, &p);
    }
    Ok(())
}

#[tauri::command(async)]
pub fn media_index_search(state: AppStateRef, query: String) -> Result<Vec<index::Hit>, String> {
    let c = state.work_cipher()?;
    let all = store::all(&state.db.lock().unwrap(), &c);
    let mut hits = index::search(all.iter().map(|e| (e.id.as_str(), e.segments.as_slice())), &query);
    hits.truncate(200);
    Ok(hits)
}

#[tauri::command(async)]
pub fn media_index_rename_speaker(state: AppStateRef, id: String, from: String, to: String) -> Result<(), String> {
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut e = store::get(&conn, &c, &id).ok_or("That file is no longer in the Media index.")?;
    store::rename_speaker(&mut e, &from, &to);
    store::put(&conn, &c, &e)?;
    drop(conn);
    crate::files::media_transcript(&state.paths.data, &e.path, &store::transcript_text(&e.segments));
    Ok(())
}

/// Captions for a recording, saved where the person chose; the format
/// follows the file name (.vtt or .srt).
#[tauri::command(async)]
pub fn media_index_save_captions(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    let c = state.work_cipher()?;
    let e = store::get(&state.db.lock().unwrap(), &c, &id).ok_or("That file is no longer in the Media index.")?;
    let cues = subs::captions(&e.segments);
    let text = if path.to_lowercase().ends_with(".vtt") { subs::to_vtt(&cues) } else { subs::to_srt(&cues) };
    std::fs::write(&path, text).map_err(|e| format!("Couldn't save the captions: {e}"))
}

// ---------- news and podcast feeds ----------

fn web_client(state: &crate::AppState) -> Result<reqwest::Client, String> {
    crate::web::client(state.settings().connectivity, false).map_err(|_| "Feeds need web access: switch to Local AI + Web (or Cloud) to read them.".to_string())
}

#[tauri::command(async)]
pub fn media_feeds(state: AppStateRef) -> Vec<feeds::Saved> {
    feeds::saved(&state.db.lock().unwrap())
}

/// Reads the feed once before saving it, so a wrong address is caught now.
#[tauri::command]
pub async fn media_feed_add(state: AppStateRef<'_>, url: String) -> Result<Vec<feeds::Saved>, String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    let url = feeds::check_url(&url)?;
    let feed = feeds::fetch(&web_client(&state)?, &url).await?;
    let podcast = feed.items.iter().any(|i| i.audio.is_some());
    let title = if feed.title.is_empty() { url.clone() } else { feed.title };
    feeds::save(&state.db.lock().unwrap(), feeds::Saved { url, title, podcast })
}

#[tauri::command(async)]
pub fn media_feed_remove(state: AppStateRef, url: String) -> Result<Vec<feeds::Saved>, String> {
    feeds::forget(&state.db.lock().unwrap(), &url)
}

#[tauri::command]
pub async fn media_feed_fetch(state: AppStateRef<'_>, url: String) -> Result<feeds::Feed, String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    feeds::fetch(&web_client(&state)?, &url).await
}

// ---------- screen history ----------

fn timeline_open(state: &crate::AppState, pin: Option<&str>) -> Result<crate::crypto::Cipher, String> {
    timeline::unlock(&state.db.lock().unwrap(), pin)?;
    state.work_cipher()
}

/// One day of captures (`day_start` in seconds, local midnight).
#[tauri::command(async)]
pub fn media_timeline_day(state: AppStateRef, pin: Option<String>, day_start: i64) -> Result<Vec<timeline::Shot>, String> {
    let c = timeline_open(&state, pin.as_deref())?;
    Ok(timeline::shots(&state.db.lock().unwrap(), &c, day_start, day_start + 86_400))
}

#[tauri::command(async)]
pub fn media_timeline_search(state: AppStateRef, pin: Option<String>, query: String) -> Result<Vec<timeline::Shot>, String> {
    let c = timeline_open(&state, pin.as_deref())?;
    Ok(timeline::search(&state.db.lock().unwrap(), &c, &query))
}

/// A capture as a data: URL for the window.
#[tauri::command(async)]
pub fn media_timeline_image(state: AppStateRef, pin: Option<String>, at: i64) -> Result<String, String> {
    use base64::Engine;
    let c = timeline_open(&state, pin.as_deref())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(timeline::image(&state, &c, at)?)))
}

#[tauri::command(async)]
pub fn media_timeline_clear(state: AppStateRef, pin: Option<String>) -> Result<(), String> {
    timeline::unlock(&state.db.lock().unwrap(), pin.as_deref())?;
    timeline::clear(&state)
}

// ---------- live captions ----------

#[tauri::command]
pub async fn media_live_captions_start(app: tauri::AppHandle, state: AppStateRef<'_>, translate: bool) -> Result<(), String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    live::start(app, state.inner().clone(), translate).await
}

#[tauri::command(async)]
pub fn media_live_captions_stop() {
    live::stop();
}

#[tauri::command(async)]
pub fn media_live_captions_save(path: String) -> Result<(), String> {
    let cues = live::cues();
    if cues.is_empty() {
        return Err("No captions to save yet.".into());
    }
    let text = if path.to_lowercase().ends_with(".vtt") { subs::to_vtt(&cues) } else { subs::to_srt(&cues) };
    std::fs::write(&path, text).map_err(|e| format!("Couldn't save the captions: {e}"))
}

// ---------- photos and screenshots ----------

const PHOTO_FOLDERS: &str = "media.photo_folders";

#[tauri::command(async)]
pub fn media_photo_folders(state: AppStateRef) -> Vec<String> {
    crate::db::get(&state.db.lock().unwrap(), PHOTO_FOLDERS).unwrap_or_default()
}

#[tauri::command(async)]
pub fn media_set_photo_folders(state: AppStateRef, folders: Vec<String>) -> Result<(), String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    let folders: Vec<String> = folders.into_iter().map(|f| f.trim().to_string()).filter(|f| std::path::Path::new(f).is_dir()).collect();
    crate::db::set(&state.db.lock().unwrap(), PHOTO_FOLDERS, &folders)
}

#[tauri::command(async)]
pub fn media_photos(state: AppStateRef) -> Vec<photos::Photo> {
    let folders: Vec<String> = crate::db::get(&state.db.lock().unwrap(), PHOTO_FOLDERS).unwrap_or_default();
    photos::scan(&folders.iter().map(std::path::PathBuf::from).collect::<Vec<_>>())
}

/// Previews only for pictures inside the chosen folders.
#[tauri::command(async)]
pub fn media_photo_thumb(state: AppStateRef, path: String, size: Option<u32>) -> Result<String, String> {
    use base64::Engine;
    let folders: Vec<String> = crate::db::get(&state.db.lock().unwrap(), PHOTO_FOLDERS).unwrap_or_default();
    let p = std::path::Path::new(&path).canonicalize().map_err(|_| "That picture isn't there.".to_string())?;
    let inside = folders.iter().filter_map(|f| std::path::Path::new(f).canonicalize().ok()).any(|f| p.starts_with(&f)) || p.starts_with(photos::screenshot_dir().canonicalize().unwrap_or_default());
    if !inside {
        return Err("That picture isn't in the Photos folders.".into());
    }
    let jpg = photos::thumb(&p, size.unwrap_or(256).clamp(32, 2048))?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
}

#[tauri::command(async)]
pub fn media_screenshot(state: AppStateRef) -> Result<String, String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    photos::screenshot().map(|p| p.to_string_lossy().to_string())
}

// ---------- the player ----------

const PLAYABLE: &[&str] = &["mp4", "webm", "mkv", "mov", "m4v", "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus"];
const MAX_PLAY: u64 = 2 * 1024 * 1024 * 1024;

pub fn playable(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| PLAYABLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// A sound or video file the person picked, for the player. Only media
/// kinds, so the window can't read other files through it.
#[tauri::command(async)]
pub fn media_play_file(state: AppStateRef, path: String) -> Result<tauri::ipc::Response, String> {
    crate::features::require(&state, crate::features::Feature::Library)?;
    let p = std::path::Path::new(&path);
    if !playable(p) {
        return Err("The player plays sound and video files.".into());
    }
    let len = std::fs::metadata(p).map_err(|_| "That file isn't there.".to_string())?.len();
    if len > MAX_PLAY {
        return Err("That file is too big for the player (over 2 GB).".into());
    }
    std::fs::read(p).map(tauri::ipc::Response::new).map_err(|e| e.to_string())
}

/// Subtitles beside a video: "Film.srt", "Film.en.srt" or "Film.vtt".
#[tauri::command(async)]
pub fn media_find_subtitles(path: String) -> Vec<String> {
    let p = std::path::Path::new(&path);
    let (Some(dir), Some(stem)) = (p.parent(), p.file_stem().map(|s| s.to_string_lossy().to_lowercase())) else { return vec![] };
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<String> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|f| {
            let name = f.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            (name.ends_with(".srt") || name.ends_with(".vtt")) && (name.starts_with(&format!("{stem}.")))
        })
        .map(|f| f.to_string_lossy().to_string())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn subtitles_beside_a_video() {
        let dir = std::env::temp_dir().join(format!("sulcus-subs-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["Film.mp4", "Film.en.srt", "Film.vtt", "Filmmaker.srt", "Other.srt"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let found = super::media_find_subtitles(dir.join("Film.mp4").to_string_lossy().to_string());
        let names: Vec<_> = found.iter().map(|f| std::path::Path::new(f).file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["Film.en.srt", "Film.vtt"]);
        assert!(super::playable(std::path::Path::new("a.MKV")) && !super::playable(std::path::Path::new("a.txt")));
        let _ = std::fs::remove_dir_all(dir);
    }
}

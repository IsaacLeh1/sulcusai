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
    let conn = state.db.lock().unwrap();
    timeline::set(&conn, settings, pin.as_deref())
}

#[tauri::command(async)]
pub fn media_timeline_set_pin(state: AppStateRef, pin: Option<String>, new_pin: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    timeline::set_pin(&conn, pin.as_deref(), &new_pin)
}

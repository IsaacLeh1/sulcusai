// SPDX-License-Identifier: AGPL-3.0-only
//! The gallery: everything made in the studio (and pictures attached to
//! chats). Files are sealed with the data key in `<data>/media`, and each
//! one's details (prompt, model, size) are an encrypted row.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::crypto::Cipher;
use crate::paths::Paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaItem {
    pub id: String,
    /// "image", "video" or "audio".
    pub kind: String,
    /// How it was made: generate, edit, fill, extend, restyle, upscale,
    /// remove_background, video, music, sound, narrate, import, screenshot,
    /// trim or paste.
    pub op: String,
    pub prompt: String,
    pub model_id: Option<String>,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    /// Length of a video or sound, in seconds.
    pub seconds: f64,
    pub size: u64,
    pub created_at: i64,
    /// What it was made from (another item).
    pub parent: Option<String>,
    /// The chat it was made in or attached to.
    pub chat_id: Option<String>,
    pub seed: Option<i64>,
    /// Song lyrics, when the music model wrote or used them.
    pub lyrics: Option<String>,
    pub favorite: bool,
    /// Chat attachments and screenshots: not listed in the studio.
    pub hidden: bool,
    /// Videos: a still frame is saved as the thumbnail.
    pub poster: bool,
}

pub fn dir(paths: &Paths) -> PathBuf {
    paths.data.join("media")
}

fn file(paths: &Paths, id: &str) -> PathBuf {
    dir(paths).join(id)
}

fn thumb_file(paths: &Paths, id: &str) -> PathBuf {
    dir(paths).join(format!("{id}.thumb"))
}

/// Ids are made here, so they're safe as file names; check anyway.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn write_sealed(path: &Path, cipher: &Cipher, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, cipher.seal_bytes(bytes)).map_err(|e| format!("Couldn't save the file: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Couldn't save the file: {e}"))
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Saves a new item: its file, an optional thumbnail, and its row.
pub fn save(conn: &Connection, paths: &Paths, cipher: &Cipher, mut item: MediaItem, bytes: &[u8], thumb: Option<&[u8]>) -> Result<MediaItem, String> {
    if item.id.is_empty() {
        item.id = new_id();
    }
    if !valid_id(&item.id) {
        return Err("Bad media id.".into());
    }
    if item.created_at == 0 {
        item.created_at = crate::db::now_ms();
    }
    item.size = bytes.len() as u64;
    std::fs::create_dir_all(dir(paths)).map_err(|e| e.to_string())?;
    write_sealed(&file(paths, &item.id), cipher, bytes)?;
    if let Some(t) = thumb {
        write_sealed(&thumb_file(paths, &item.id), cipher, t)?;
    }
    let json = serde_json::to_string(&item).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO media (id, kind, chat_id, hidden, data, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![item.id, item.kind, item.chat_id, item.hidden as i64, cipher.encrypt(&json), item.created_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(item)
}

fn row(cipher: &Cipher, data: String) -> Option<MediaItem> {
    serde_json::from_str(&cipher.decrypt(&data).ok()?).ok()
}

/// The studio's items, newest first (not chat attachments).
pub fn list(conn: &Connection, cipher: &Cipher, kind: Option<&str>, limit: usize, before: Option<i64>) -> Vec<MediaItem> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT data FROM media WHERE hidden = 0 AND (?1 IS NULL OR kind = ?1) AND (?2 IS NULL OR created_at < ?2)
         ORDER BY created_at DESC LIMIT ?3",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![kind, before, limit as i64], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).filter_map(|d| row(cipher, d)).collect())
        .unwrap_or_default()
}

pub fn get(conn: &Connection, cipher: &Cipher, id: &str) -> Option<MediaItem> {
    let data: String = conn.query_row("SELECT data FROM media WHERE id = ?1", [id], |r| r.get(0)).optional().ok().flatten()?;
    row(cipher, data)
}

pub fn update(conn: &Connection, cipher: &Cipher, item: &MediaItem) -> Result<(), String> {
    let json = serde_json::to_string(item).map_err(|e| e.to_string())?;
    conn.execute("UPDATE media SET data = ?2, hidden = ?3 WHERE id = ?1", params![item.id, cipher.encrypt(&json), item.hidden as i64])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn read(paths: &Paths, cipher: &Cipher, id: &str) -> Result<Vec<u8>, String> {
    if !valid_id(id) {
        return Err("Bad media id.".into());
    }
    let blob = std::fs::read(file(paths, id)).map_err(|_| "That file is missing.".to_string())?;
    cipher.open_bytes(&blob)
}

/// Replaces an item's thumbnail.
pub fn write_thumb(paths: &Paths, cipher: &Cipher, id: &str, bytes: &[u8]) -> Result<(), String> {
    if !valid_id(id) {
        return Err("Bad media id.".into());
    }
    write_sealed(&thumb_file(paths, id), cipher, bytes)
}

pub fn read_thumb(paths: &Paths, cipher: &Cipher, id: &str) -> Result<Vec<u8>, String> {
    if !valid_id(id) {
        return Err("Bad media id.".into());
    }
    match std::fs::read(thumb_file(paths, id)) {
        Ok(blob) => cipher.open_bytes(&blob),
        Err(_) => read(paths, cipher, id),
    }
}

pub fn delete(conn: &Connection, paths: &Paths, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("Bad media id.".into());
    }
    std::fs::remove_file(file(paths, id)).ok();
    std::fs::remove_file(thumb_file(paths, id)).ok();
    conn.execute("DELETE FROM media WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    Ok(())
}

/// A deleted chat takes its attachments (pictures pasted into it,
/// screenshots) with it; things made in it stay in the studio.
pub fn delete_attachments(conn: &Connection, paths: &Paths, chat_id: &str) {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM media WHERE chat_id = ?1 AND hidden = 1")
        .and_then(|mut s| s.query_map([chat_id], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    for id in ids {
        let _ = delete(conn, paths, &id);
    }
}

/// The newest picture made in or attached to a chat ("edit it" means this one).
pub fn latest_in_chat(conn: &Connection, cipher: &Cipher, chat_id: &str, kind: &str) -> Option<MediaItem> {
    let data: String = conn
        .query_row("SELECT data FROM media WHERE chat_id = ?1 AND kind = ?2 ORDER BY created_at DESC LIMIT 1", params![chat_id, kind], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    row(cipher, data)
}

pub fn count(conn: &Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM media WHERE hidden = 0", [], |r| r.get::<_, i64>(0)).map(|n| n as usize).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_sealed_and_listed_newest_first() {
        let d = std::env::temp_dir().join(format!("sulcusai-media-{}", uuid::Uuid::new_v4()));
        let paths = Paths::new(d.clone()).unwrap();
        let conn = crate::db::open(&paths.db).unwrap();
        let cipher = Cipher::for_test();
        let a = save(&conn, &paths, &cipher, MediaItem { kind: "image".into(), prompt: "a red fox".into(), created_at: 1, ..Default::default() }, b"PNGDATA", Some(b"THUMB")).unwrap();
        let b = save(&conn, &paths, &cipher, MediaItem { kind: "audio".into(), prompt: "a song".into(), created_at: 2, ..Default::default() }, b"MP3", None).unwrap();
        save(&conn, &paths, &cipher, MediaItem { kind: "image".into(), hidden: true, chat_id: Some("c1".into()), ..Default::default() }, b"X", None).unwrap();

        // Nothing readable on disk.
        let raw = std::fs::read(dir(&paths).join(&a.id)).unwrap();
        assert!(!raw.windows(7).any(|w| w == b"PNGDATA"));
        let row: String = conn.query_row("SELECT data FROM media WHERE id = ?1", [&a.id], |r| r.get(0)).unwrap();
        assert!(!row.contains("red fox"));

        assert_eq!(read(&paths, &cipher, &a.id).unwrap(), b"PNGDATA");
        assert_eq!(read_thumb(&paths, &cipher, &a.id).unwrap(), b"THUMB");
        assert_eq!(read_thumb(&paths, &cipher, &b.id).unwrap(), b"MP3", "no thumbnail: the file itself");
        let all = list(&conn, &cipher, None, 10, None);
        assert_eq!(all.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![b.id.as_str(), a.id.as_str()], "attachments aren't listed");
        assert_eq!(list(&conn, &cipher, Some("image"), 10, None).len(), 1);
        assert_eq!(count(&conn), 2);

        delete_attachments(&conn, &paths, "c1");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM media", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        delete(&conn, &paths, &a.id).unwrap();
        assert!(get(&conn, &cipher, &a.id).is_none());
        assert!(read(&paths, &cipher, "../keys.json").is_err());
        std::fs::remove_dir_all(d).ok();
    }
}

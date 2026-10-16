// SPDX-License-Identifier: AGPL-3.0-only
//! Handoff notes between chats. A chat that leaves a note for "the next
//! session" (HANDOFF.md, next-steps.md…) in a shared folder was being picked
//! up by unrelated chats. While "Chats work together" is off, a chat reads
//! another chat's handoff note only when the person says so: in their
//! message ("pick up where the other chat left off") or by allowing it on a
//! card. With "Chats work together" on, chats share freely.
//!
//! Which chat wrote which note is kept (paths encrypted, keyed by a hash),
//! with the chats allowed to read each: Settings → Handoffs between chats.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::Cipher;
use crate::{db, AppState, AppStateRef};

const KEY: &str = "handoffs";
/// Notes remembered at most (the oldest go first).
const MAX_NOTES: usize = 500;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Book {
    notes: Vec<Note>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Note {
    /// SHA-256 of the lowercased path.
    key: String,
    /// The path, encrypted.
    path: String,
    /// The chat that wrote it.
    chat: String,
    at: i64,
    /// Chats the person let read it.
    allowed: Vec<String>,
}

fn key_of(path: &Path) -> String {
    let p = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    hex::encode(Sha256::digest(p.display().to_string().to_lowercase().as_bytes()))
}

fn book(conn: &rusqlite::Connection) -> Book {
    db::get(conn, KEY).unwrap_or_default()
}

static NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)hand[-_ ]?off|hand[-_ ]?over|next[-_ ]?(steps|session)|pick[-_ ]?up|continu(e|ation)|session[-_ ]?(notes|log|summary)|progress[-_ ]?(notes|log)|agent[-_ ]?notes|context[-_ ]?notes|where[-_ ]?(i|we)[-_ ]?left").unwrap()
});
static HEAD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)hand[- ]?off|hand[- ]?over|for the next (chat|session|agent|assistant)|next session|where (i|we) left off|pick (this|it) up|continue from here").unwrap()
});

/// A note left for whoever works on this next: by its name, or its opening.
pub fn looks_like_handoff(path: &Path, head: &str) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    NAME.is_match(&name) || HEAD.is_match(&head.chars().take(600).collect::<String>())
}

/// Whether chats may share work ("Chats work together").
fn together(conn: &rusqlite::Connection) -> bool {
    crate::features::is_on(conn, crate::features::Feature::ChatTeam)
}

/// A chat wrote a file: remembered when it looks like a handoff note.
pub fn wrote(state: &AppState, cipher: &Cipher, chat_id: &str, path: &Path, content: &str) {
    if !looks_like_handoff(path, content) {
        return;
    }
    let conn = state.db.lock().unwrap();
    let mut b = book(&conn);
    let key = key_of(path);
    b.notes.retain(|n| n.key != key);
    b.notes.push(Note { key, path: cipher.encrypt(&path.display().to_string()), chat: chat_id.to_string(), at: db::now_ms(), allowed: Vec::new() });
    let extra = b.notes.len().saturating_sub(MAX_NOTES);
    b.notes.drain(..extra);
    let _ = db::set(&conn, KEY, &b);
}

/// Another chat's handoff note that this chat may not read yet: the title of
/// the chat that left it. None when chats work together, when it isn't a
/// handoff note, when this chat wrote it, or when the person allowed it.
pub fn guarded(state: &AppState, cipher: &Cipher, chat_id: &str, path: &Path) -> Option<String> {
    let conn = state.db.lock().unwrap();
    if together(&conn) {
        return None;
    }
    let key = key_of(path);
    let b = book(&conn);
    let known = b.notes.iter().find(|n| n.key == key);
    match known {
        Some(n) if n.chat == chat_id || n.allowed.iter().any(|a| a == chat_id) => None,
        Some(n) => Some(db::chat(&conn, cipher, &n.chat).map_or_else(|| "another chat".to_string(), |c| c.title)),
        None => {
            // Not from a chat here (or from before this was kept): judged by name and opening.
            drop(conn);
            let head = std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b[..b.len().min(2000)]).into_owned()).unwrap_or_default();
            looks_like_handoff(path, &head).then(|| "another chat or app".to_string())
        }
    }
}

/// The person let this chat read the note.
pub fn allow(state: &AppState, cipher: &Cipher, chat_id: &str, path: &Path) {
    let conn = state.db.lock().unwrap();
    let mut b = book(&conn);
    let key = key_of(path);
    match b.notes.iter_mut().find(|n| n.key == key) {
        Some(n) => {
            if !n.allowed.iter().any(|a| a == chat_id) {
                n.allowed.push(chat_id.to_string());
            }
        }
        None => b.notes.push(Note { key, path: cipher.encrypt(&path.display().to_string()), chat: String::new(), at: db::now_ms(), allowed: vec![chat_id.to_string()] }),
    }
    let _ = db::set(&conn, KEY, &b);
}

static PICK_UP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(pick(ing)? (it |this |that )?up|continue|carry on|resume|take over|keep going)\b").unwrap()
});
static FROM_OTHER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)hand[- ]?off|hand[- ]?over|left off|(other|previous|last|earlier|old) (chat|session|conversation)|another chat|where .{0,40} (stopped|ended)").unwrap()
});

/// The person's own words ask this chat to pick up another chat's work.
pub fn asked_to_pick_up(text: &str) -> bool {
    PICK_UP.is_match(text) && FROM_OTHER.is_match(text)
}

/// A command that names a handoff note (reading it another way).
pub fn command_names_one(command: &str) -> bool {
    command.split(|c: char| c.is_whitespace() || "\"'`;|&<>(),".contains(c)).any(|w| !w.is_empty() && NAME.is_match(w.rsplit(['/', '\\']).next().unwrap_or(w)))
}

// ---------- the manager ----------

#[derive(Serialize)]
pub struct NoteView {
    key: String,
    path: String,
    /// The chat that wrote it, if it still exists.
    by: Option<String>,
    at: i64,
    allowed: Vec<String>,
}

#[derive(Serialize)]
pub struct HandoffsView {
    /// "Chats work together" is on: the guard is off.
    together: bool,
    notes: Vec<NoteView>,
}

fn view(state: &AppState, cipher: &Cipher) -> HandoffsView {
    let conn = state.db.lock().unwrap();
    let title = |id: &str| db::chat(&conn, cipher, id).map(|c| c.title);
    let notes = book(&conn)
        .notes
        .iter()
        .rev()
        .map(|n| NoteView {
            key: n.key.clone(),
            path: cipher.decrypt(&n.path).unwrap_or_default(),
            by: title(&n.chat),
            at: n.at,
            allowed: n.allowed.iter().map(|a| title(a).unwrap_or_else(|| "a deleted chat".into())).collect(),
        })
        .collect();
    HandoffsView { together: together(&conn), notes }
}

#[tauri::command(async)]
pub fn handoffs_view(state: AppStateRef) -> Result<HandoffsView, String> {
    let c = state.cipher()?;
    Ok(view(&state, &c))
}

/// Takes back every chat's permission to read a note.
#[tauri::command(async)]
pub fn handoff_revoke(state: AppStateRef, key: String) -> Result<HandoffsView, String> {
    let c = state.cipher()?;
    {
        let conn = state.db.lock().unwrap();
        let mut b = book(&conn);
        if let Some(n) = b.notes.iter_mut().find(|n| n.key == key) {
            n.allowed.clear();
        }
        db::set(&conn, KEY, &b)?;
    }
    state.log("privacy", "Took back chats' permission to read a handoff note");
    Ok(view(&state, &c))
}

/// Forgets a note (it's no longer treated as a handoff by its writer).
#[tauri::command(async)]
pub fn handoff_forget(state: AppStateRef, key: String) -> Result<HandoffsView, String> {
    let c = state.cipher()?;
    {
        let conn = state.db.lock().unwrap();
        let mut b = book(&conn);
        b.notes.retain(|n| n.key != key);
        db::set(&conn, KEY, &b)?;
    }
    Ok(view(&state, &c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_notes_are_known_by_name_or_opening() {
        assert!(looks_like_handoff(Path::new("C:/p/HANDOFF.md"), ""));
        assert!(looks_like_handoff(Path::new("next-steps.md"), ""));
        assert!(looks_like_handoff(Path::new("notes.md"), "# Handoff\nWhere we left off: the login page."));
        assert!(!looks_like_handoff(Path::new("index.html"), "<html>"));
        assert!(!looks_like_handoff(Path::new("README.md"), "# My app\nRun it with npm start."));
    }

    #[test]
    fn only_clear_words_count_as_asking_to_pick_up() {
        assert!(asked_to_pick_up("Pick up where the other chat left off on the bakery site"));
        assert!(asked_to_pick_up("continue from the handoff"));
        assert!(asked_to_pick_up("Can you take over the previous chat's work?"));
        assert!(!asked_to_pick_up("Make me a website for my bakery"));
        assert!(!asked_to_pick_up("continue"), "a bare continue is about this chat");
        assert!(!asked_to_pick_up("what's in the handoff folder?"));
        assert!(command_names_one("Get-Content .\\HANDOFF.md"));
        assert!(command_names_one("cat docs/next-steps.md | head"));
        assert!(!command_names_one("npm run dev"));
    }
}

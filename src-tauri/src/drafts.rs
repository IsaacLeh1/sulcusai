// SPDX-License-Identifier: AGPL-3.0-only
//! Messages typed in a chat and not sent yet. The window keeps them while
//! SulcusAI runs; with "Keep unsent messages after a restart" on (off by
//! default) they're also saved here, encrypted like the chats, until sent
//! or cleared.

use std::collections::HashMap;

use crate::AppStateRef;

const KEEP: &str = "drafts_keep";
const DRAFTS: &str = "drafts";

fn read(conn: &rusqlite::Connection, c: &crate::crypto::Cipher) -> HashMap<String, String> {
    crate::db::get::<String>(conn, DRAFTS).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write(conn: &rusqlite::Connection, c: &crate::crypto::Cipher, all: &HashMap<String, String>) -> Result<(), String> {
    crate::db::set(conn, DRAFTS, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

/// Whether unsent messages are kept after a restart.
#[tauri::command]
pub fn drafts_keep(state: AppStateRef) -> bool {
    crate::db::get::<bool>(&state.db.lock().unwrap(), KEEP).unwrap_or(false)
}

/// Turns keeping them after a restart on or off; off forgets the saved ones.
#[tauri::command]
pub fn set_drafts_keep(state: AppStateRef, on: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    crate::db::set(&conn, KEEP, &on)?;
    if !on {
        crate::db::set(&conn, DRAFTS, &String::new())?;
    }
    Ok(())
}

/// The saved unsent messages, by chat (none while keeping them is off).
#[tauri::command]
pub fn drafts(state: AppStateRef) -> Result<HashMap<String, String>, String> {
    if !drafts_keep(state.clone()) {
        return Ok(HashMap::new());
    }
    let c = state.cipher()?;
    Ok(read(&state.db.lock().unwrap(), &c))
}

/// Saves (or, empty, clears) a chat's unsent message, when keeping is on.
#[tauri::command]
pub fn draft_set(state: AppStateRef, chat_id: String, text: String) -> Result<(), String> {
    if !drafts_keep(state.clone()) {
        return Ok(());
    }
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = read(&conn, &c);
    if text.trim().is_empty() {
        all.remove(&chat_id);
    } else {
        all.insert(chat_id, text.chars().take(100_000).collect());
    }
    write(&conn, &c, &all)
}

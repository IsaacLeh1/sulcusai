// SPDX-License-Identifier: AGPL-3.0-only
//! Chats hidden from the sidebar: the user's own (by id) and chats others
//! share with them ("contact:chat"). They're still there, open and
//! searchable, and listed under "Hidden chats" to show again.

use crate::{db, AppStateRef};

const KEY: &str = "hidden_chats";

pub fn list(conn: &rusqlite::Connection) -> Vec<String> {
    db::get(conn, KEY).unwrap_or_default()
}

#[tauri::command(async)]
pub fn hidden_chats(state: AppStateRef) -> Result<Vec<String>, String> {
    state.cipher()?;
    Ok(list(&state.db.lock().unwrap()))
}

/// Hides a chat from the sidebar, or shows it again.
#[tauri::command(async)]
pub fn set_chat_hidden(state: AppStateRef, key: String, hidden: bool) -> Result<Vec<String>, String> {
    state.cipher()?;
    let key = key.trim().to_string();
    if key.is_empty() || key.len() > 200 {
        return Err("That isn't a chat.".into());
    }
    let conn = state.db.lock().unwrap();
    let mut all = list(&conn);
    all.retain(|k| k != &key);
    if hidden {
        all.push(key);
    }
    db::set(&conn, KEY, &all)?;
    db::log_action(&conn, "chat", if hidden { "Hid a chat from the sidebar" } else { "Showed a hidden chat in the sidebar again" });
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_and_showing_keeps_one_entry() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        let set = |all: &mut Vec<String>, key: &str, hidden: bool| {
            all.retain(|k| k != key);
            if hidden {
                all.push(key.to_string());
            }
            db::set(&conn, KEY, all).unwrap();
        };
        let mut all = list(&conn);
        set(&mut all, "chat-1", true);
        set(&mut all, "contact-9:chat-2", true);
        set(&mut all, "chat-1", true);
        assert_eq!(list(&conn), vec!["contact-9:chat-2".to_string(), "chat-1".to_string()]);
        set(&mut all, "chat-1", false);
        assert_eq!(list(&conn), vec!["contact-9:chat-2".to_string()]);
    }
}

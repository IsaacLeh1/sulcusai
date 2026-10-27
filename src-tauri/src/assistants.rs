// SPDX-License-Identifier: AGPL-3.0-only
//! Assistants of the person's own: a name, instructions, a model and a few
//! ways to start, kept to start chats with ("my email writer", "a Spanish
//! tutor"). A chat started with one keeps its instructions on top of the
//! app's own. Kept encrypted like chats.

use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::crypto::Cipher;
use crate::{db, AppStateRef};

const KEY: &str = "assistants";
/// Which chat was started with which assistant (ids only).
const CHATS_KEY: &str = "chat_assistants";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Assistant {
    pub id: String,
    pub name: String,
    /// One line on what it's for, shown in the list.
    pub description: String,
    pub instructions: String,
    /// The model its chats start on (None: the default model).
    pub model: Option<String>,
    /// Messages offered in a new chat with it.
    pub starters: Vec<String>,
}

fn load(conn: &Connection, c: &Cipher) -> Vec<Assistant> {
    db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
}

fn store(conn: &Connection, c: &Cipher, all: &[Assistant]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

fn chats(conn: &Connection) -> HashMap<String, String> {
    db::get(conn, CHATS_KEY).unwrap_or_default()
}

/// The assistant a chat was started with, if it's still there.
pub fn of_chat(conn: &Connection, c: &Cipher, chat_id: &str) -> Option<Assistant> {
    let id = chats(conn).remove(chat_id)?;
    load(conn, c).into_iter().find(|a| a.id == id)
}

/// The instructions with the chat's assistant's added.
pub fn with_instructions(base: String, a: &Assistant) -> String {
    let own = a.instructions.trim();
    if own.is_empty() {
        return base;
    }
    format!("{base}\n\nIn this chat you are the user's own assistant “{}”. Its instructions, from the user:\n{own}", a.name.trim())
}

#[tauri::command(async)]
pub fn assistants(state: AppStateRef) -> Result<Vec<Assistant>, String> {
    let c = state.cipher()?;
    Ok(load(&state.db.lock().unwrap(), &c))
}

/// Adds one (`id` empty) or changes one; gives them all back.
#[tauri::command(async)]
pub fn save_assistant(state: AppStateRef, assistant: Assistant) -> Result<Vec<Assistant>, String> {
    let c = state.cipher()?;
    let mut a = assistant;
    a.name = a.name.trim().chars().take(60).collect();
    a.starters = a.starters.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(6).collect();
    if a.name.is_empty() {
        return Err("Give the assistant a name.".into());
    }
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn, &c);
    if all.iter().any(|x| x.name.eq_ignore_ascii_case(&a.name) && x.id != a.id) {
        return Err("Another assistant has that name.".into());
    }
    match all.iter_mut().find(|x| x.id == a.id && !a.id.is_empty()) {
        Some(x) => *x = a,
        None => {
            a.id = uuid::Uuid::new_v4().to_string();
            all.push(a);
        }
    }
    store(&conn, &c, &all)?;
    Ok(all)
}

#[tauri::command(async)]
pub fn delete_assistant(state: AppStateRef, id: String) -> Result<Vec<Assistant>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn, &c);
    all.retain(|a| a.id != id);
    store(&conn, &c, &all)?;
    let mut map = chats(&conn);
    map.retain(|_, a| *a != id);
    db::set(&conn, CHATS_KEY, &map)?;
    Ok(all)
}

/// A new chat with an assistant, on its model (when still installed).
#[tauri::command(async)]
pub fn chat_with_assistant(state: AppStateRef, id: String) -> Result<db::Chat, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let a = load(&conn, &c).into_iter().find(|a| a.id == id).ok_or("That assistant was removed.")?;
    let model = a.model.filter(|m| db::installed_model(&conn, m).is_some() || crate::cloud::is_cloud(m));
    let mut chat = db::create_chat(&conn, &c, model)?;
    db::set_chat_title(&conn, &c, &chat.id, &a.name)?;
    chat.title = a.name.clone();
    let mut map = chats(&conn);
    // Chats deleted since are let go of.
    let live: std::collections::HashSet<String> = db::list_chats(&conn, &c).into_iter().map(|ch| ch.id).collect();
    map.retain(|chat_id, _| live.contains(chat_id));
    map.insert(chat.id.clone(), a.id);
    db::set(&conn, CHATS_KEY, &map)?;
    Ok(chat)
}

/// The assistant a chat was started with.
#[tauri::command(async)]
pub fn chat_assistant(state: AppStateRef, chat_id: String) -> Result<Option<Assistant>, String> {
    let c = state.cipher()?;
    Ok(of_chat(&state.db.lock().unwrap(), &c, &chat_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chats_assistant_adds_its_instructions() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = Cipher::random();
        let a = Assistant { id: "a1".into(), name: "Spanish tutor".into(), instructions: "Answer in simple Spanish, then in English.".into(), ..Default::default() };
        store(&conn, &c, std::slice::from_ref(&a)).unwrap();
        db::set(&conn, CHATS_KEY, &HashMap::from([("chat1".to_string(), "a1".to_string())])).unwrap();
        assert_eq!(of_chat(&conn, &c, "chat1"), Some(a.clone()));
        assert_eq!(of_chat(&conn, &c, "other"), None);
        let base = with_instructions("Be helpful.".into(), &a);
        assert!(base.starts_with("Be helpful.") && base.contains("Spanish tutor") && base.ends_with("then in English."));
        let raw: String = db::get(&conn, KEY).unwrap();
        assert!(!raw.contains("Spanish"), "kept encrypted");
    }
}

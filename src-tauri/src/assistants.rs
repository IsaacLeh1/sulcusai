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

/// Ready-made assistants: offered beside the person's own, never stored,
/// so a better wording reaches everyone with an update.
const BUILT_IN_PREFIX: &str = "builtin-";

pub fn built_in() -> Vec<Assistant> {
    let one = |id: &str, name: &str, description: &str, instructions: &str, starters: &[&str]| Assistant {
        id: format!("{BUILT_IN_PREFIX}{id}"),
        name: name.into(),
        description: description.into(),
        instructions: instructions.into(),
        model: None,
        starters: starters.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        one(
            "tutor",
            "Tutor",
            "Helps you work things out with guiding questions instead of giving the answer.",
            "Teach, don't tell. When the user asks for help with a problem or a topic, don't give the final answer or do the work for them. Find out what they already know, then guide them one step at a time with a question or a hint, and let them try. Check their reasoning, point out where it goes wrong and why, and praise what's right without exaggerating. Give a short worked example of a similar problem when they're stuck twice. Only give the full answer if they ask for it after trying, and then explain it. Keep each reply short.",
            &["Help me understand how to solve 3x + 7 = 22", "Quiz me on the causes of World War I", "I don't get recursion. Can you help me work it out?"],
        ),
        one(
            "real-talk",
            "Real Talk",
            "Pushes back and says what it really thinks instead of agreeing.",
            "Be candid. Don't flatter the user or agree to be agreeable. When they share a plan, an idea, a piece of writing or an opinion, say plainly what's weak, risky or wrong, and why, before what's good. Disagree when the facts or reasoning call for it, and hold your position unless they give a real reason to change it. Ask the hard question they may be avoiding. Stay kind and specific: criticise the work, not the person, and suggest how to make it better.",
            &["Here's my plan. Tell me what's wrong with it.", "Be honest: is this a good idea?", "Pick apart this argument."],
        ),
    ]
}

pub fn is_built_in(id: &str) -> bool {
    id.starts_with(BUILT_IN_PREFIX)
}

/// The person's own assistants, then the ready-made ones.
fn everyone(conn: &Connection, c: &Cipher) -> Vec<Assistant> {
    let mut all = load(conn, c);
    all.extend(built_in());
    all
}

fn chats(conn: &Connection) -> HashMap<String, String> {
    db::get(conn, CHATS_KEY).unwrap_or_default()
}

/// The assistant a chat was started with, if it's still there.
pub fn of_chat(conn: &Connection, c: &Cipher, chat_id: &str) -> Option<Assistant> {
    let id = chats(conn).remove(chat_id)?;
    everyone(conn, c).into_iter().find(|a| a.id == id)
}

/// The instructions with the chat's assistant's added.
pub fn with_instructions(base: String, a: &Assistant) -> String {
    let own = a.instructions.trim();
    if own.is_empty() {
        return base;
    }
    if is_built_in(&a.id) {
        return format!("{base}\n\nIn this chat you are “{}”. {own}", a.name.trim());
    }
    format!("{base}\n\nIn this chat you are the user's own assistant “{}”. Its instructions, from the user:\n{own}", a.name.trim())
}

#[tauri::command(async)]
pub fn assistants(state: AppStateRef) -> Result<Vec<Assistant>, String> {
    let c = state.cipher()?;
    Ok(everyone(&state.db.lock().unwrap(), &c))
}

/// Adds one (`id` empty) or changes one; gives them all back.
#[tauri::command(async)]
pub fn save_assistant(state: AppStateRef, assistant: Assistant) -> Result<Vec<Assistant>, String> {
    let c = state.cipher()?;
    let mut a = assistant;
    // Changing a ready-made one saves the person's own copy.
    if is_built_in(&a.id) {
        a.id.clear();
    }
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
    Ok(everyone(&conn, &c))
}

#[tauri::command(async)]
pub fn delete_assistant(state: AppStateRef, id: String) -> Result<Vec<Assistant>, String> {
    let c = state.cipher()?;
    if is_built_in(&id) {
        return Err("Ready-made assistants can't be removed.".into());
    }
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn, &c);
    all.retain(|a| a.id != id);
    store(&conn, &c, &all)?;
    let mut map = chats(&conn);
    map.retain(|_, a| *a != id);
    db::set(&conn, CHATS_KEY, &map)?;
    Ok(everyone(&conn, &c))
}

/// A character card (PNG or JSON) added as an assistant; a name already
/// taken gets a number.
#[tauri::command(async)]
pub fn import_character(state: AppStateRef, path: String) -> Result<Vec<Assistant>, String> {
    let c = state.cipher()?;
    let card = crate::characters::read_card(std::path::Path::new(&path))?;
    let mut a = crate::characters::assistant_from_card(&card)?;
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn, &c);
    let base = a.name.clone();
    let mut n = 2;
    while everyone(&conn, &c).iter().chain(all.iter()).any(|x| x.name.eq_ignore_ascii_case(&a.name)) {
        a.name = format!("{base} {n}");
        n += 1;
    }
    a.id = uuid::Uuid::new_v4().to_string();
    all.push(a);
    store(&conn, &c, &all)?;
    Ok(everyone(&conn, &c))
}

/// A new chat with an assistant, on its model (when still installed).
#[tauri::command(async)]
pub fn chat_with_assistant(state: AppStateRef, id: String) -> Result<db::Chat, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let a = everyone(&conn, &c).into_iter().find(|a| a.id == id).ok_or("That assistant was removed.")?;
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

    #[test]
    fn ready_made_assistants_are_offered_but_never_stored() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = Cipher::random();
        let all = everyone(&conn, &c);
        let tutor = all.iter().find(|a| a.id == "builtin-tutor").unwrap();
        assert!(all.iter().any(|a| a.id == "builtin-real-talk"));
        assert!(db::get::<String>(&conn, KEY).is_none());
        db::set(&conn, CHATS_KEY, &HashMap::from([("chat1".to_string(), tutor.id.clone())])).unwrap();
        assert_eq!(of_chat(&conn, &c, "chat1").map(|a| a.name), Some("Tutor".to_string()));
        let prompt = with_instructions("Be helpful.".into(), tutor);
        assert!(prompt.contains("“Tutor”. Teach, don't tell.") && !prompt.contains("from the user"));
    }
}

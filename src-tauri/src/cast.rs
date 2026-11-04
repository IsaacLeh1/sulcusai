// SPDX-License-Identifier: AGPL-3.0-only
//! Group chats with characters: a chat started with one assistant can have
//! a few more join. After each reply, each of them answers in turn, in their
//! own voice, from the conversation so far. They only talk; tools stay with
//! the chat's own assistant.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use crate::assistants::{self, Assistant};
use crate::{db, AppState, AppStateRef};

const KEY: &str = "chat_cast";
/// Characters that may join one chat besides its own.
pub const MAX_CAST: usize = 4;
const TRANSCRIPT_CHARS: usize = 10_000;

fn casts(conn: &rusqlite::Connection) -> HashMap<String, Vec<String>> {
    db::get(conn, KEY).unwrap_or_default()
}

pub fn cast_of(conn: &rusqlite::Connection, chat_id: &str) -> Vec<String> {
    casts(conn).remove(chat_id).unwrap_or_default()
}

/// The conversation as named lines, newest kept.
pub(crate) fn transcript(history: &[db::Message]) -> String {
    let mut out = String::new();
    for m in history.iter().rev().filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty()) {
        let line = if m.role == "user" { format!("User: {}\n", m.content.trim()) } else { format!("{}\n", m.content.trim()) };
        if out.chars().count() + line.chars().count() > TRANSCRIPT_CHARS {
            break;
        }
        out.insert_str(0, &line);
    }
    out
}

pub(crate) fn messages_for(member: &Assistant, others: &[String], history: &[db::Message]) -> Vec<serde_json::Value> {
    let name = member.name.trim();
    let base = format!(
        "This is a group chat between the user and several characters: {}. You are {name}. Write only {name}'s next message, in their own voice, reacting to what was just said. Keep it short, don't speak for anyone else, and don't start with your name.",
        others.join(", ")
    );
    let recent: String = history.iter().rev().take(4).map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
    let system = format!("{}{}", assistants::with_instructions(base, member), assistants::lore_for(member, &recent));
    vec![json!({ "role": "system", "content": system }), json!({ "role": "user", "content": format!("The chat so far:\n{}\n\nNow {name}'s message:", transcript(history)) })]
}

/// Each joined character answers after the main reply; returns the last
/// message saved (for the window), or None when nobody joined.
pub async fn follow_ups(state: &Arc<AppState>, ep: &crate::engine::Endpoint, chat_id: &str, cancel: &std::sync::atomic::AtomicBool) -> Option<db::Message> {
    let c = state.cipher().ok()?;
    let (members, own) = {
        let conn = state.db.lock().unwrap();
        let ids = cast_of(&conn, chat_id);
        if ids.is_empty() {
            return None;
        }
        let all = assistants::everyone_pub(&conn, &c);
        let own = assistants::of_chat(&conn, &c, chat_id);
        (ids.iter().filter_map(|id| all.iter().find(|a| a.id == *id).cloned()).collect::<Vec<_>>(), own)
    };
    let names: Vec<String> = own.iter().chain(members.iter()).map(|a| a.name.trim().to_string()).collect();
    let mut last = None;
    for member in members.iter().filter(|m| own.as_ref().is_none_or(|o| o.id != m.id)) {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let history = db::messages(&state.db.lock().unwrap(), &c, chat_id);
        let Ok(raw) = crate::chat::complete(ep, messages_for(member, &names, &history), json!({ "temperature": 0.8, "chat_template_kwargs": { "enable_thinking": false } }), 500).await else { continue };
        let text = crate::chat::strip_thinking(&raw).trim().to_string();
        if text.is_empty() {
            continue;
        }
        let m = db::Message {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: chat_id.to_string(),
            role: "assistant".into(),
            content: format!("**{}:** {text}", member.name.trim()),
            created_at: db::now_ms(),
            ..Default::default()
        };
        if db::add_message(&state.db.lock().unwrap(), &c, &m).is_ok() {
            last = Some(m);
        }
    }
    last
}

#[tauri::command(async)]
pub fn chat_cast(state: AppStateRef, chat_id: String) -> Vec<String> {
    cast_of(&state.db.lock().unwrap(), &chat_id)
}

#[tauri::command(async)]
pub fn set_chat_cast(state: AppStateRef, chat_id: String, ids: Vec<String>) -> Result<Vec<String>, String> {
    let mut ids: Vec<String> = ids.into_iter().filter(|i| !i.trim().is_empty()).collect();
    ids.dedup();
    if ids.len() > MAX_CAST {
        return Err(format!("Up to {MAX_CAST} characters can join a chat."));
    }
    let conn = state.db.lock().unwrap();
    let mut all = casts(&conn);
    if ids.is_empty() {
        all.remove(&chat_id);
    } else {
        all.insert(chat_id.clone(), ids.clone());
    }
    db::set(&conn, KEY, &all)?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a real model (SULCUSAI_TEST_GGUF): a joined character answers in
    /// its own voice without speaking for the others.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn e2e_a_joined_character_answers_for_itself() {
        let (_engine, ep, name) = crate::e2e::test_model().await;
        let msg = |role: &str, content: &str| db::Message { role: role.into(), content: content.into(), ..Default::default() };
        let vale = Assistant { id: "v".into(), name: "Vale".into(), instructions: "A gruff retired sky-pirate who runs a tea shop and loves puns.".into(), ..Default::default() };
        let history = [msg("user", "Where should we sail?"), msg("assistant", "**Mira:** North, to the ice fields. The maps say there's a lost city.")];
        let raw = crate::chat::complete(&ep, messages_for(&vale, &["Mira".into(), "Vale".into()], &history), json!({ "temperature": 0.8, "chat_template_kwargs": { "enable_thinking": false } }), 500).await.unwrap();
        let text = crate::chat::strip_thinking(&raw).trim().to_string();
        println!("{name}: {text}");
        assert!(!text.is_empty() && !text.contains("Mira:") && !text.starts_with("Vale:") && !text.contains("User:"));
    }

    #[test]
    fn each_character_gets_the_chat_and_speaks_only_for_itself() {
        let msg = |role: &str, content: &str| db::Message { role: role.into(), content: content.into(), ..Default::default() };
        let vale = Assistant { id: "v".into(), name: "Vale".into(), instructions: "A retired sky-pirate.".into(), ..Default::default() };
        let history = [msg("user", "Where should we sail?"), msg("assistant", "**Mira:** North, to the ice.")];
        let m = messages_for(&vale, &["Mira".into(), "Vale".into()], &history);
        let system = m[0]["content"].as_str().unwrap();
        assert!(system.contains("You are Vale") && system.contains("Mira, Vale") && system.contains("sky-pirate"));
        let asked = m[1]["content"].as_str().unwrap();
        assert!(asked.contains("User: Where should we sail?") && asked.contains("**Mira:** North") && asked.ends_with("Now Vale's message:"));
    }
}

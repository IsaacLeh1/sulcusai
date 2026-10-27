// SPDX-License-Identifier: AGPL-3.0-only
//! Searching every chat from the sidebar, and saving one chat as a file.
//! Messages are encrypted in the database, so the search reads them here,
//! one chat at a time, and nothing leaves this PC.

use std::path::Path;

use serde::Serialize;

use crate::crypto::Cipher;
use crate::db::{self, Chat};
use crate::{AppState, AppStateRef};

/// A chat that has every word searched for.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatHit {
    pub chat_id: String,
    pub title: String,
    /// Where in it the words are (none when only the name has them).
    pub snippet: Option<String>,
    pub updated_at: i64,
}

const MOST: usize = 40;

/// Chats with every word of `query` in their name or messages: names
/// first, then the latest.
pub fn search(state: &AppState, c: &Cipher, query: &str) -> Vec<ChatHit> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let chats = db::list_chats(&state.db.lock().unwrap(), c);
    let (mut named, mut said) = (Vec::new(), Vec::new());
    for chat in chats.into_iter().filter(|ch| !ch.empty) {
        if named.len() + said.len() >= MOST {
            break;
        }
        // The lock is taken for each chat, so a long search doesn't hold up replies.
        let messages = db::messages(&state.db.lock().unwrap(), c, &chat.id);
        if let Some(hit) = matches(&chat, &messages, &words) {
            if hit.snippet.is_none() || words.iter().all(|w| chat.title.to_lowercase().contains(w)) {
                named.push(hit);
            } else {
                said.push(hit);
            }
        }
    }
    named.extend(said);
    named
}

fn matches(chat: &Chat, messages: &[db::Message], words: &[String]) -> Option<ChatHit> {
    let title = chat.title.to_lowercase();
    let texts: Vec<(&db::Message, String)> = messages
        .iter()
        .filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty())
        .map(|m| (m, m.content.to_lowercase()))
        .collect();
    if !words.iter().all(|w| title.contains(w) || texts.iter().any(|(_, t)| t.contains(w))) {
        return None;
    }
    // The message with the most of the words, around the first of them in it.
    let best = texts
        .iter()
        .map(|(m, t)| (words.iter().filter(|w| t.contains(w.as_str())).count(), m))
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, _)| *n);
    let snippet = best.map(|(_, m)| {
        let low = m.content.to_lowercase();
        let first = words.iter().filter(|w| low.contains(w.as_str())).min_by_key(|w| low.find(w.as_str())).map(String::as_str).unwrap_or("");
        crate::tools::chats::snippet(&m.content, first)
    });
    Some(ChatHit { chat_id: chat.id.clone(), title: chat.title.clone(), snippet, updated_at: chat.updated_at })
}

#[tauri::command]
pub async fn search_chats(state: AppStateRef<'_>, query: String) -> Result<Vec<ChatHit>, String> {
    let state = state.inner().clone();
    let c = state.cipher()?;
    tauri::async_runtime::spawn_blocking(move || search(&state, &c, &query)).await.map_err(|e| e.to_string())
}

/// One chat as Markdown: who said what, with when.
pub fn chat_markdown(chat: &Chat, messages: &[db::Message]) -> String {
    let when = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
    let mut md = format!("# {}\n\n_Started {}_\n\n", chat.title, when(chat.created_at));
    for m in messages {
        match m.role.as_str() {
            "user" => md.push_str(&format!("## You ({})\n\n{}\n\n", when(m.created_at), m.content.trim())),
            "assistant" if !m.content.trim().is_empty() => md.push_str(&format!("## Assistant\n\n{}\n\n", m.content.trim())),
            _ => {}
        }
    }
    md
}

/// Saves a chat to `path`: as Markdown (.md), or as the web page the
/// window made of it (.html, given as `html`).
#[tauri::command(async)]
pub fn export_chat(state: AppStateRef, chat_id: String, path: String, html: Option<String>) -> Result<(), String> {
    let c = state.cipher()?;
    let path = Path::new(&path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let text = match (ext.as_str(), html) {
        ("md" | "markdown" | "txt", _) => {
            let conn = state.db.lock().unwrap();
            let chat = db::chat(&conn, &c, &chat_id).ok_or("That chat no longer exists.")?;
            chat_markdown(&chat, &db::messages(&conn, &c, &chat_id))
        }
        ("html" | "htm", Some(html)) => html,
        _ => return Err("Save it as a .md or .html file.".into()),
    };
    std::fs::write(path, text).map_err(|e| format!("Couldn't save it there: {e}"))?;
    state.log("chat", &format!("Saved a chat as {}", path.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> db::Message {
        db::Message { role: role.into(), content: text.into(), ..Default::default() }
    }

    /// Every word must be somewhere in the chat; the excerpt comes from the
    /// message with the most of them.
    #[test]
    fn chats_need_every_word() {
        let chat = Chat { id: "c".into(), title: "Trip to Japan".into(), ..Default::default() };
        let messages = [msg("user", "Where should we stay in Kyoto?"), msg("assistant", "In Kyoto, Gion is lovely in spring, near the old temples."), msg("tool", "kyoto spring secret")];
        let words = |q: &str| q.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let hit = matches(&chat, &messages, &words("kyoto spring")).unwrap();
        assert!(hit.snippet.unwrap().contains("Gion is lovely"));
        assert!(matches(&chat, &messages, &words("japan kyoto")).is_some(), "the name counts");
        assert!(matches(&chat, &messages, &words("kyoto secret")).is_none(), "tool output doesn't");
        assert_eq!(matches(&chat, &messages, &words("trip")).unwrap().snippet, None);
    }
}

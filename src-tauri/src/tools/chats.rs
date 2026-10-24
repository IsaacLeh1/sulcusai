// SPDX-License-Identifier: AGPL-3.0-only
//! Chats working together on this PC: the assistant can look through the
//! user's other chats (never incognito ones), read one, and ask another
//! chat's assistant something (`message_chat`, run by the agent: the other
//! chat answers there, with the model that's loaded, and its reply comes
//! back). A chat answering for another can't pass the question on again.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};

use super::{arg_str, Ctx, Outcome};
use crate::crypto::Cipher;
use crate::db::{self, Chat};

pub fn search_chats_params() -> Value {
    json!({ "type": "object", "properties": {
        "query": { "type": "string", "description": "Words to look for in chat titles and messages. Leave it out to list the most recent chats." } } })
}

pub fn read_chat_params() -> Value {
    json!({ "type": "object", "required": ["chat"], "properties": {
        "chat": { "type": "string", "description": "The chat's id or title, from search_chats." },
        "last": { "type": "integer", "description": "How many of its latest messages to read (default 20)." } } })
}

pub fn message_chat_params() -> Value {
    json!({ "type": "object", "required": ["message"], "properties": {
        "chat": { "type": "string", "description": "The chat's id or title, from search_chats." },
        "new_chat_title": { "type": "string", "description": "Instead of an existing chat: start a new one with this title (for a separate piece of work)." },
        "message": { "type": "string", "description": "What to ask or tell that chat's assistant. It can't see this conversation, so include what it needs." } } })
}

/// Chats answering a question from another chat right now.
static ASKED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Whether this chat is answering for another one (so it can't ask on).
pub fn is_asked(chat_id: &str) -> bool {
    ASKED.lock().unwrap().contains(chat_id)
}

/// Marks a chat as answering for another while it's held.
pub struct Asked(String);
impl Asked {
    pub fn new(chat_id: &str) -> Asked {
        ASKED.lock().unwrap().insert(chat_id.to_string());
        Asked(chat_id.to_string())
    }
}
impl Drop for Asked {
    fn drop(&mut self) {
        ASKED.lock().unwrap().remove(&self.0);
    }
}

/// The user's other chats: not incognito, not empty, not this one.
fn others(conn: &rusqlite::Connection, c: &Cipher, this: &str) -> Vec<Chat> {
    db::list_chats(conn, c).into_iter().filter(|ch| ch.id != this && !ch.incognito && db::message_count(conn, &ch.id) > 0).collect()
}

/// A chat by id, exact title, or the only title containing it.
pub fn find(conn: &rusqlite::Connection, c: &Cipher, this: &str, wanted: &str) -> Result<Chat, String> {
    let wanted = wanted.trim();
    let all = others(conn, c, this);
    if let Some(ch) = all.iter().find(|ch| ch.id == wanted || ch.title.eq_ignore_ascii_case(wanted)) {
        return Ok(ch.clone());
    }
    let low = wanted.to_lowercase();
    let near: Vec<&Chat> = all.iter().filter(|ch| ch.title.to_lowercase().contains(&low)).collect();
    match near.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("There's no other chat called \"{wanted}\". Use search_chats to find it.")),
        many => Err(format!(
            "More than one chat matches \"{wanted}\": {}. Use its id.",
            many.iter().take(5).map(|c| format!("\"{}\" ({})", c.title, c.id)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// A short excerpt around the first match.
fn snippet(text: &str, low_query: &str) -> String {
    let low = text.to_lowercase();
    let Some(at) = low.find(low_query) else { return text.chars().take(120).collect() };
    let chars: Vec<char> = text.chars().collect();
    let pos = low[..at].chars().count();
    let start = pos.saturating_sub(50);
    let end = (pos + low_query.chars().count() + 70).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    s.replace('\n', " ")
}

pub fn search(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let (db, c, this) = (ctx.memory.db, ctx.memory.cipher, ctx.memory.chat_id);
    let query = args.get("query").and_then(Value::as_str).map(str::trim).unwrap_or("");
    let conn = db.lock().unwrap();
    let chats = others(&conn, c, this);
    let mut lines = Vec::new();
    if query.is_empty() {
        for ch in chats.iter().take(15) {
            lines.push(format!("- \"{}\" (id {}, {} messages)", ch.title, ch.id, db::message_count(&conn, &ch.id)));
        }
    } else {
        let low = query.to_lowercase();
        for ch in &chats {
            let in_title = ch.title.to_lowercase().contains(&low);
            let hit = db::messages(&conn, c, &ch.id)
                .into_iter()
                .filter(|m| m.role == "user" || m.role == "assistant")
                .find(|m| m.content.to_lowercase().contains(&low));
            if in_title || hit.is_some() {
                let found = hit.map(|m| format!(": {}: {}", if m.role == "user" { "user" } else { "assistant" }, snippet(&m.content, &low))).unwrap_or_default();
                lines.push(format!("- \"{}\" (id {}){found}", ch.title, ch.id));
            }
            if lines.len() >= 10 {
                break;
            }
        }
    }
    let title = if query.is_empty() { "Listed the other chats".to_string() } else { format!("Searched the other chats for \"{query}\"") };
    if lines.is_empty() {
        let text = if chats.is_empty() { "There are no other chats yet.".to_string() } else { format!("No other chat mentions \"{query}\".") };
        return Outcome::ok(text, title, "text", None);
    }
    let text = format!("{}\nRead one with read_chat, or ask its assistant with message_chat.", lines.join("\n"));
    Outcome::ok(text.clone(), title, "text", Some(text))
}

pub fn read(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let (db, c, this) = (ctx.memory.db, ctx.memory.cipher, ctx.memory.chat_id);
    let wanted = match arg_str(args, "chat") {
        Ok(w) => w,
        Err(e) => return Outcome::error("Read a chat", e),
    };
    let last = args.get("last").and_then(Value::as_u64).unwrap_or(20).clamp(1, 100) as usize;
    let conn = db.lock().unwrap();
    let chat = match find(&conn, c, this, wanted) {
        Ok(ch) => ch,
        Err(e) => return Outcome::error("Read a chat", e),
    };
    let said: Vec<String> = db::messages(&conn, c, &chat.id)
        .into_iter()
        .filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty())
        .map(|m| format!("{}: {}", if m.role == "user" { "User" } else { "Assistant" }, m.content.trim()))
        .collect();
    let from = said.len().saturating_sub(last);
    let mut out = format!("Chat \"{}\" (id {}), its last {} messages:\n\n{}", chat.title, chat.id, said.len() - from, said[from..].join("\n\n"));
    if from > 0 {
        out.push_str(&format!("\n\n({from} earlier messages not shown.)"));
    }
    let mut o = Outcome::ok(out, format!("Read the chat \"{}\"", chat.title), "text", None);
    o.meta["open_chat"] = json!(chat.id);
    o
}

pub fn delete_chat_params() -> Value {
    json!({ "type": "object", "required": ["chat"], "properties": {
        "chat": { "type": "string", "description": "The chat's id or title, from search_chats. Only a chat the user asked you to delete." } } })
}

/// The card the user sees before a chat is deleted: which one, and how much is in it.
pub fn delete_preview(args: &Value, db: &std::sync::Mutex<rusqlite::Connection>, c: &Cipher, this: &str) -> Result<super::Preview, String> {
    let conn = db.lock().unwrap();
    let chat = find(&conn, c, this, arg_str(args, "chat")?)?;
    let n = db::message_count(&conn, &chat.id);
    let when = chrono::DateTime::from_timestamp_millis(chat.updated_at).map(|d| d.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string()).unwrap_or_default();
    Ok(super::Preview {
        title: format!("Delete the chat “{}”", chat.title),
        kind: "text",
        detail: Some(format!("{n} messages, last used {when}.")),
        note: Some("Deleted for good, with its messages and pasted pictures. This can't be undone.".into()),
    })
}

/// Deletes another chat (the user approved it on the card): never this one,
/// an incognito one, or one that's working right now.
pub fn delete(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let (db, c, this) = (ctx.memory.db, ctx.memory.cipher, ctx.memory.chat_id);
    let title = "Delete a chat";
    if is_asked(this) {
        return Outcome::error(title, "A chat answering for another can't delete chats.");
    }
    let chat = {
        let conn = db.lock().unwrap();
        match arg_str(args, "chat").and_then(|w| find(&conn, c, this, w)) {
            Ok(ch) => ch,
            Err(e) => return Outcome::error(title, e),
        }
    };
    if ctx.state.generations.lock().unwrap().contains_key(&chat.id) {
        return Outcome::error(title, format!("“{}” is working right now. Stop it first, then try again.", chat.title));
    }
    if let Err(e) = crate::delete_chat_now(ctx.state, &chat.id) {
        return Outcome::error(title, e);
    }
    if let Some(app) = ctx.app {
        use tauri::Emitter;
        app.emit("chat:deleted", json!({ "chat_id": chat.id })).ok();
    }
    Outcome::ok(format!("Deleted the chat “{}”.", chat.title), format!("Deleted the chat “{}”", chat.title), "text", None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snippet_shows_the_words_around_the_match() {
        let text = format!("{} the invoice for Acme is due Friday {}", "x".repeat(200), "y".repeat(200));
        let s = snippet(&text, "invoice");
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("invoice for Acme"));
        assert_eq!(snippet("short note", "zzz"), "short note");
    }
}

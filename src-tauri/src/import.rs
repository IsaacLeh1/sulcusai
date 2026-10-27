// SPDX-License-Identifier: AGPL-3.0-only
//! Bringing chats and saved memories over from other AI apps.
//!
//! - ChatGPT: Settings › Data controls › Export data mails a zip; its
//!   conversations.json has every chat as a tree of messages (edits and
//!   regenerations are branches); the branch the person last saw is kept.
//! - Claude: Settings › Privacy › Export data mails a zip; its
//!   conversations.json has every chat's messages in order. Memories, when
//!   the export has them (a "memories" file), come over as memories.
//! - Memories from any assistant can also be pasted as text: one per line.
//!
//! Each conversation becomes a chat here with its title, its dates and its
//! text (pictures and files aren't in the exports), in a folder named after
//! the app it came from. Nothing is sent anywhere.

use std::io::Read;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::{db, AppState, AppStateRef};

#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub title: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// (role "user" or "assistant", text, when in ms).
    pub messages: Vec<(String, String, i64)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Export {
    /// "ChatGPT" or "Claude".
    pub from: String,
    pub conversations: Vec<Conversation>,
    pub memories: Vec<String>,
}

fn secs_ms(v: &Value) -> Option<i64> {
    v.as_f64().map(|s| (s * 1000.0) as i64)
}

fn iso_ms(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|d| d.timestamp_millis())
}

/// ChatGPT's conversation: the branch from its current node back to the
/// root, in order, with what people and the assistant wrote.
fn chatgpt_conversation(c: &Value) -> Option<Conversation> {
    let mapping = c.get("mapping")?.as_object()?;
    let mut node = c.get("current_node").and_then(Value::as_str).map(str::to_string)?;
    let mut messages = Vec::new();
    let mut guard = 0;
    while let Some(n) = mapping.get(&node) {
        guard += 1;
        if guard > 100_000 {
            break;
        }
        if let Some(m) = n.get("message").filter(|m| !m.is_null()) {
            let role = m.pointer("/author/role").and_then(Value::as_str).unwrap_or_default();
            let hidden = m.pointer("/metadata/is_visually_hidden_from_conversation").and_then(Value::as_bool).unwrap_or(false);
            let text = m.pointer("/content/parts").and_then(Value::as_array).map(|parts| parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default();
            if matches!(role, "user" | "assistant") && !hidden && !text.trim().is_empty() {
                messages.push((role.to_string(), text.trim().to_string(), m.get("create_time").and_then(secs_ms).unwrap_or(0)));
            }
        }
        match n.get("parent").and_then(Value::as_str) {
            Some(p) => node = p.to_string(),
            None => break,
        }
    }
    messages.reverse();
    if messages.is_empty() {
        return None;
    }
    let created = c.get("create_time").and_then(secs_ms).unwrap_or(messages[0].2);
    Some(Conversation {
        title: c.get("title").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).unwrap_or("Chat from ChatGPT").to_string(),
        created_ms: created,
        updated_ms: c.get("update_time").and_then(secs_ms).unwrap_or(created),
        messages,
    })
}

/// Claude's conversation: its messages in order.
fn claude_conversation(c: &Value) -> Option<Conversation> {
    let mut messages = Vec::new();
    for m in c.get("chat_messages")?.as_array()? {
        let role = match m.get("sender").and_then(Value::as_str) {
            Some("human") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let from_parts = m.get("content").and_then(Value::as_array).map(|parts| {
            parts.iter().filter(|p| p.get("type").and_then(Value::as_str) == Some("text")).filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
        });
        let text = from_parts.filter(|t| !t.trim().is_empty()).or_else(|| m.get("text").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
        if !text.trim().is_empty() {
            messages.push((role.to_string(), text.trim().to_string(), m.get("created_at").and_then(iso_ms).unwrap_or(0)));
        }
    }
    if messages.is_empty() {
        return None;
    }
    let created = c.get("created_at").and_then(iso_ms).unwrap_or(messages[0].2);
    Some(Conversation {
        title: c.get("name").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).unwrap_or("Chat from Claude").to_string(),
        created_ms: created,
        updated_ms: c.get("updated_at").and_then(iso_ms).unwrap_or(created),
        messages,
    })
}

/// The conversations in an export's conversations.json, whichever app made it.
pub fn conversations(json: &Value) -> Result<(String, Vec<Conversation>), String> {
    let list = json.as_array().ok_or("That file isn't a list of conversations.")?;
    let chatgpt = list.iter().any(|c| c.get("mapping").is_some());
    let claude = list.iter().any(|c| c.get("chat_messages").is_some());
    let (from, convs): (&str, Vec<Conversation>) = if chatgpt {
        ("ChatGPT", list.iter().filter_map(chatgpt_conversation).collect())
    } else if claude {
        ("Claude", list.iter().filter_map(claude_conversation).collect())
    } else if list.is_empty() {
        return Err("That export has no conversations.".into());
    } else {
        return Err("That doesn't look like a ChatGPT or Claude export (conversations.json).".into());
    };
    Ok((from.to_string(), convs))
}

/// Memories in pasted text or an export's memories file: one a line, with
/// list marks taken off.
pub fn memory_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '•', '·']).trim())
        .map(|l| l.split_once(". ").filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty()).map_or(l, |(_, rest)| rest))
        .filter(|l| l.chars().filter(|c| c.is_alphanumeric()).count() >= 6)
        .map(|l| l.chars().take(500).collect())
        .collect()
}

/// Every string in a JSON value (an export's memories file, whatever its shape).
fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(o) => o.iter().filter(|(k, _)| !matches!(k.as_str(), "uuid" | "id" | "created_at" | "updated_at" | "account_uuid")).for_each(|(_, x)| strings(x, out)),
        _ => {}
    }
}

/// An export: a zip as the app mails it, or its conversations.json.
pub fn read(path: &Path) -> Result<Export, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read that file: {e}"))?;
    let (convs_json, memories_json) = if bytes.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).map_err(|e| format!("Couldn't open that zip: {e}"))?;
        let mut read_entry = |want: &dyn Fn(&str) -> bool| -> Option<String> {
            let name = (0..zip.len()).filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string())).find(|n| want(&n.to_lowercase()))?;
            let mut s = String::new();
            zip.by_name(&name).ok()?.read_to_string(&mut s).ok()?;
            Some(s)
        };
        let convs = read_entry(&|n| n.ends_with("conversations.json")).ok_or("That zip has no conversations.json: is it a ChatGPT or Claude export?")?;
        let mems = read_entry(&|n| n.ends_with(".json") && n.rsplit('/').next().is_some_and(|f| f.starts_with("memor")));
        (convs, mems)
    } else {
        (String::from_utf8_lossy(&bytes).into_owned(), None)
    };
    let json: Value = serde_json::from_str(&convs_json).map_err(|_| "That file isn't readable JSON.".to_string())?;
    let (from, conversations) = conversations(&json)?;
    let mut memories = Vec::new();
    if let Some(m) = memories_json.and_then(|m| serde_json::from_str::<Value>(&m).ok()) {
        let mut all = Vec::new();
        strings(&m, &mut all);
        memories = all.iter().flat_map(|s| memory_lines(s)).collect();
    }
    Ok(Export { from, conversations, memories })
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Imported {
    pub from: String,
    pub chats: usize,
    pub messages: usize,
    pub memories: usize,
    /// Chats already here from an earlier import of the same export.
    pub already: usize,
}

/// Saves an export's chats (skipping ones imported before: same title and
/// start time) and memories.
pub fn save(state: &AppState, export: &Export) -> Result<Imported, String> {
    let c = state.cipher()?;
    let mut done = Imported { from: export.from.clone(), ..Default::default() };
    let mut made = Vec::new();
    {
        let conn = state.db.lock().unwrap();
        let seen: std::collections::HashSet<(String, i64)> = db::list_chats(&conn, &c).into_iter().map(|ch| (ch.title, ch.created_at)).collect();
        for conv in &export.conversations {
            if seen.contains(&(conv.title.clone(), conv.created_ms)) {
                done.already += 1;
                continue;
            }
            let chat = db::create_chat(&conn, &c, None)?;
            db::set_chat_title(&conn, &c, &chat.id, &conv.title)?;
            for (i, (role, text, at)) in conv.messages.iter().enumerate() {
                let created_at = if *at > 0 { *at } else { conv.created_ms + i as i64 };
                db::add_message(&conn, &c, &db::Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: role.clone(), content: text.clone(), created_at, ..Default::default() })?;
                done.messages += 1;
            }
            db::set_chat_times(&conn, &chat.id, conv.created_ms, conv.updated_ms)?;
            made.push(chat.id);
            done.chats += 1;
        }
        for m in &export.memories {
            if crate::memory::looks_secret(m) {
                continue;
            }
            if crate::memory::add(&conn, &c, m, None, None).is_ok_and(|(_, new)| new) {
                done.memories += 1;
            }
        }
        db::log_action(&conn, "import", &format!("Brought {} chats ({} messages) and {} memories over from {}", done.chats, done.messages, done.memories, done.from));
    }
    if !made.is_empty() {
        crate::folders::put_chats_in(state, &format!("From {}", export.from), &made)?;
    }
    Ok(done)
}

// ---------- commands ----------

/// Imports a ChatGPT or Claude export the person picked.
#[tauri::command(async)]
pub fn import_chats(state: AppStateRef, path: String) -> Result<Imported, String> {
    let export = read(Path::new(&path))?;
    save(&state, &export)
}

/// Memories pasted from another assistant: one a line.
#[tauri::command(async)]
pub fn import_memories(state: AppStateRef, text: String) -> Result<usize, String> {
    let lines = memory_lines(&text);
    if lines.is_empty() {
        return Err("Paste the memories, one on each line.".into());
    }
    let done = save(&state, &Export { from: "another assistant".into(), conversations: Vec::new(), memories: lines })?;
    Ok(done.memories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_chatgpt_export_keeps_the_branch_last_seen() {
        let export = json!([{
            "title": "Trip to Japan", "create_time": 1700000000.0, "update_time": 1700000100.0, "current_node": "c",
            "mapping": {
                "root": { "id": "root", "message": null, "parent": null, "children": ["s"] },
                "s": { "id": "s", "message": { "author": { "role": "system" }, "content": { "content_type": "text", "parts": [""] } }, "parent": "root", "children": ["u"] },
                "u": { "id": "u", "message": { "author": { "role": "user" }, "create_time": 1700000001.0, "content": { "content_type": "text", "parts": ["Plan 5 days in Kyoto"] } }, "parent": "s", "children": ["a1", "a2"] },
                "a1": { "id": "a1", "message": { "author": { "role": "assistant" }, "content": { "content_type": "text", "parts": ["An older answer"] } }, "parent": "u", "children": [] },
                "a2": { "id": "a2", "message": { "author": { "role": "assistant" }, "create_time": 1700000002.0, "content": { "content_type": "text", "parts": ["Day 1: Fushimi Inari"] } }, "parent": "u", "children": ["c"] },
                "c": { "id": "c", "message": { "author": { "role": "user" }, "create_time": 1700000003.0, "content": { "content_type": "multimodal_text", "parts": [{ "content_type": "image_asset_pointer" }, "And food?"] } }, "parent": "a2", "children": [] }
            }
        }]);
        let (from, convs) = conversations(&export).unwrap();
        assert_eq!(from, "ChatGPT");
        let texts: Vec<&str> = convs[0].messages.iter().map(|m| m.1.as_str()).collect();
        assert_eq!(texts, ["Plan 5 days in Kyoto", "Day 1: Fushimi Inari", "And food?"], "the branch last seen, pictures left out");
        assert_eq!(convs[0].created_ms, 1_700_000_000_000);
    }

    #[test]
    fn a_claude_export_reads_in_order() {
        let export = json!([{ "uuid": "x", "name": "Cover letter", "created_at": "2026-05-01T10:00:00Z", "updated_at": "2026-05-01T11:00:00Z", "chat_messages": [
            { "sender": "human", "text": "Write a cover letter", "content": [{ "type": "text", "text": "Write a cover letter" }], "created_at": "2026-05-01T10:00:01Z" },
            { "sender": "assistant", "text": "", "content": [{ "type": "text", "text": "Dear hiring manager," }, { "type": "tool_use" }], "created_at": "2026-05-01T10:00:05Z" }
        ] }, { "uuid": "y", "name": "", "chat_messages": [] }]);
        let (from, convs) = conversations(&export).unwrap();
        assert_eq!(from, "Claude");
        assert_eq!(convs.len(), 1, "an empty chat is left out");
        assert_eq!(convs[0].messages[1], ("assistant".to_string(), "Dear hiring manager,".to_string(), 1_777_629_605_000));
    }

    #[test]
    fn pasted_memories_are_one_a_line() {
        let text = "- Lives in Provo, Utah\n2. Prefers metric units\n• Has a dog named Max\nok\n\n* Works on a local AI app called SulcusAI";
        assert_eq!(memory_lines(text), ["Lives in Provo, Utah", "Prefers metric units", "Has a dog named Max", "Works on a local AI app called SulcusAI"]);
        assert!(conversations(&json!({ "not": "a list" })).is_err());
        assert!(conversations(&json!([{ "something": 1 }])).is_err());
    }
}

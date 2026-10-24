// SPDX-License-Identifier: AGPL-3.0-only
//! Cross-chat memory: short facts the assistant saves (or the user adds),
//! stored encrypted, and recalled into later chats when they're relevant.

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::crypto::Cipher;
use crate::db::now_ms;

pub const MAX_LEN: usize = 500;
/// The setting: lasting facts from chats saved without being asked (on unless turned off).
pub const AUTO_KEY: &str = "memory_auto";

pub fn auto_on(conn: &Connection) -> bool {
    crate::db::get::<bool>(conn, AUTO_KEY).unwrap_or(true)
}
/// Memories recalled into one chat turn.
const RECALL: usize = 8;
/// The newest memories are always included, matched or not.
const ALWAYS_RECENT: usize = 3;

#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub project_id: Option<String>,
    /// The chat it was saved from (None: added by the user on the Memory page).
    pub source_chat: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// All memories, or those visible in one project (its own plus global ones).
pub fn list(conn: &Connection, c: &Cipher, project: Option<&str>) -> Vec<Memory> {
    let Ok(mut stmt) = conn.prepare("SELECT id, content, project_id, created_at, updated_at, source_chat FROM memories ORDER BY updated_at DESC") else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(Memory {
            id: r.get(0)?,
            content: c.decrypt_or(&r.get::<_, String>(1)?, ""),
            project_id: r.get(2)?,
            source_chat: r.get(5)?,
            created_at: r.get(3)?,
            updated_at: r.get(4)?,
        })
    })
    .map(|rows| {
        rows.filter_map(Result::ok)
            .filter(|m| !m.content.is_empty())
            .filter(|m| project.is_none() || m.project_id.is_none() || m.project_id.as_deref() == project)
            .collect()
    })
    .unwrap_or_default()
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(['.', '!']).to_lowercase()
}

/// Saves a memory. A near-duplicate of an existing one updates it instead.
/// Returns the memory and whether it was new.
pub fn add(conn: &Connection, c: &Cipher, content: &str, project_id: Option<&str>, source_chat: Option<&str>) -> Result<(Memory, bool), String> {
    let content = content.trim();
    if content.is_empty() {
        return Err("Nothing to remember.".into());
    }
    if content.chars().count() > MAX_LEN {
        return Err(format!("Keep memories under {MAX_LEN} characters; save the key fact only."));
    }
    let key = normalize(content);
    let now = now_ms();
    for m in list(conn, c, None).into_iter().filter(|m| m.project_id.as_deref() == project_id) {
        let other = normalize(&m.content);
        if other == key || other.contains(&key) || key.contains(&other) {
            // The longer wording wins; it usually has more detail.
            let keep = if content.len() >= m.content.len() { content } else { m.content.as_str() }.to_string();
            conn.execute("UPDATE memories SET content = ?2, updated_at = ?3 WHERE id = ?1", params![m.id, c.encrypt(&keep), now]).map_err(err)?;
            return Ok((Memory { content: keep, updated_at: now, ..m }, false));
        }
    }
    let m = Memory {
        id: uuid::Uuid::new_v4().to_string(),
        content: content.to_string(),
        project_id: project_id.map(str::to_string),
        source_chat: source_chat.map(str::to_string),
        created_at: now,
        updated_at: now,
    };
    conn.execute(
        "INSERT INTO memories (id, content, project_id, source_chat, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![m.id, c.encrypt(&m.content), m.project_id, source_chat, now],
    )
    .map_err(err)?;
    Ok((m, true))
}

pub fn update(conn: &Connection, c: &Cipher, id: &str, content: &str) -> Result<(), String> {
    let content = content.trim();
    if content.is_empty() || content.chars().count() > MAX_LEN {
        return Err(format!("A memory needs 1 to {MAX_LEN} characters."));
    }
    let n = conn.execute("UPDATE memories SET content = ?2, updated_at = ?3 WHERE id = ?1", params![id, c.encrypt(content), now_ms()]).map_err(err)?;
    if n == 0 {
        return Err("That memory no longer exists.".into());
    }
    Ok(())
}

/// The memory `old` most likely means (the same words, or most of them), for
/// a fact that changed: None when nothing is close enough.
pub fn closest<'a>(memories: &'a [Memory], old: &str) -> Option<&'a Memory> {
    let key = normalize(old);
    if let Some(m) = memories.iter().find(|m| normalize(&m.content) == key) {
        return Some(m);
    }
    let want = words(old);
    if want.is_empty() {
        return None;
    }
    memories
        .iter()
        .map(|m| (words(&m.content).intersection(&want).count() as f64 / want.len() as f64, m))
        .filter(|(share, _)| *share >= 0.6)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, m)| m)
}

/// The question for the model after a reply: what (if anything) from this
/// exchange is worth keeping, given what's already kept. Its answer is JSON
/// (`AUTO_FORMAT`), read by `auto_facts`.
pub fn auto_prompt(known: &[Memory], user: &str, reply: &str) -> Vec<serde_json::Value> {
    let cut = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let list = if known.is_empty() { "(nothing yet)".to_string() } else { known.iter().take(40).map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n") };
    vec![
        serde_json::json!({ "role": "system", "content": "You keep an assistant's long-term memory about the person it helps. Read the exchange and list what's worth \
            remembering in future chats: lasting facts about the person, their work, their projects, their preferences or their decisions. Leave out passwords, \
            codes, keys and account numbers; one-off details of this task; general knowledge; anything the memory already says. Most exchanges have nothing \
            worth keeping: then the list is empty. Each fact is one short sentence in the third person (\"The user ...\"), at most 2. `about` is \"project\" \
            for a fact about the project the chat belongs to, else \"user\"." }),
        serde_json::json!({ "role": "user", "content": format!("Already remembered:\n{list}\n\nThe user wrote:\n{}\n\nThe assistant answered:\n{}", cut(user, 2000), cut(reply, 2000)) }),
    ]
}

/// The answer's shape: `{"remember": [{"fact": "...", "about": "user"}]}`.
pub fn auto_format() -> serde_json::Value {
    let item = serde_json::json!({ "type": "object", "properties": { "fact": { "type": "string" }, "about": { "type": "string", "enum": ["user", "project"] } }, "required": ["fact", "about"] });
    let schema = serde_json::json!({ "type": "object", "properties": { "remember": { "type": "array", "maxItems": 2, "items": item } }, "required": ["remember"] });
    serde_json::json!({ "type": "json_schema", "json_schema": { "name": "memories", "schema": schema } })
}

/// The facts in the model's answer, and whether each is about the user (else
/// the project): none that look secret, too long or empty, at most 2.
pub fn auto_facts(raw: &str) -> Vec<(String, bool)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else { return Vec::new() };
    v["remember"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let fact = f["fact"].as_str()?.trim().to_string();
            (!fact.is_empty() && fact.chars().count() <= 300 && !looks_secret(&fact)).then(|| (fact, f["about"].as_str() != Some("project")))
        })
        .take(2)
        .collect()
}

/// Something that shouldn't be kept: a password, key or code, or a long
/// number (an account or card).
pub fn looks_secret(text: &str) -> bool {
    let low = text.to_lowercase();
    let words = ["password", "passcode", "api key", "secret", "token", "pin code", "social security", "card number", "account number"];
    let digits = text.chars().fold((0usize, 0usize), |(run, most), c| if c.is_ascii_digit() { (run + 1, most.max(run + 1)) } else { (0, most) }).1;
    words.iter().any(|w| low.contains(w)) || digits >= 8
}

pub fn delete(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM memories WHERE id = ?1", [id]).map_err(err)?;
    Ok(())
}

pub fn clear(conn: &Connection) -> Result<usize, String> {
    conn.execute("DELETE FROM memories", []).map_err(err)
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "at", "for", "with", "is", "are", "was", "were", "be", "it",
    "this", "that", "i", "you", "me", "my", "your", "we", "do", "does", "did", "can", "could", "would", "should", "what", "how",
    "about", "from", "as", "by", "so", "if", "not", "no", "yes", "please", "user", "user's", "they", "their", "them", "have", "has",
];

fn words(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(w))
        // Crude stemming so "projects" matches "project".
        .map(|w| w.trim_end_matches("ing").trim_end_matches('s').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// The memories most related to `query` (word overlap), plus the newest few.
pub fn relevant<'a>(memories: &'a [Memory], query: &str) -> Vec<&'a Memory> {
    let q = words(query);
    let mut scored: Vec<(usize, usize, &Memory)> = memories
        .iter()
        .enumerate()
        .map(|(i, m)| (words(&m.content).intersection(&q).count(), i, m))
        .collect();
    // Most overlap first; among equals, newest first (the list is newest-first).
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut picked: Vec<&Memory> = scored.iter().filter(|(s, _, _)| *s > 0).take(RECALL).map(|(_, _, m)| *m).collect();
    for m in memories.iter().take(ALWAYS_RECENT) {
        if picked.len() >= RECALL + ALWAYS_RECENT {
            break;
        }
        if !picked.iter().any(|p| p.id == m.id) {
            picked.push(m);
        }
    }
    picked
}

/// Instructions plus recalled memories, for the system prompt.
pub fn prompt_section(recalled: &[&Memory]) -> String {
    let mut s = String::from(
        "\n\n## Memory\nYou remember things across chats. When the user shares a lasting fact about themselves, their work, their \
         projects or their preferences, or asks you to remember something, save it with the remember tool: one short sentence in \
         the third person (\"The user swing trades tech stocks.\"), one fact at a time. When a fact you remember has changed, save \
         the new one with replaces set to the old one. Don't save passwords, codes, keys or account numbers, details that only \
         matter for this chat, or what you already remember.",
    );
    if !recalled.is_empty() {
        s.push_str("\nWhat you remember from earlier chats:");
        for m in recalled {
            s.push_str(&format!("\n- {}", m.content));
        }
    }
    s
}

/// What it remembers, as a note on the newest message (models on this PC):
/// empty when nothing is recalled.
pub fn recall_note(recalled: &[&Memory]) -> String {
    if recalled.is_empty() {
        return String::new();
    }
    let list: Vec<String> = recalled.iter().map(|m| format!("- {}", m.content)).collect();
    format!("\n\n(A note from SulcusAI, not the user: what you remember from earlier chats, in case it helps.\n{})", list.join("\n"))
}

/// The conversation with the note on the message `turn_id` (the request
/// being answered). Earlier messages go as they were, so the engine reuses
/// what it already read.
pub fn with_recall(history: &[crate::db::Message], turn_id: &str, note: &str) -> Vec<crate::db::Message> {
    let mut out = history.to_vec();
    if !note.is_empty() {
        if let Some(m) = out.iter_mut().find(|m| m.id == turn_id && m.role == "user") {
            m.content.push_str(note);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Protector, Vault};

    struct Plain;
    impl Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }

    fn setup() -> (Connection, Cipher) {
        let d = std::env::temp_dir().join(format!("sulcusai-mem-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let conn = crate::db::open(&d.join("t.db")).unwrap();
        let c = Vault::open(&d.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        (conn, c)
    }

    fn m(id: &str, content: &str) -> Memory {
        Memory { id: id.into(), content: content.into(), project_id: None, source_chat: None, created_at: 0, updated_at: 0 }
    }

    #[test]
    fn a_model_remembers_a_little_and_never_secrets() {
        // What the model said is worth keeping: two at most, secrets left out.
        let raw = r#"{"remember": [
            {"fact": "Isaac's brother swing trades tech stocks.", "about": "user"},
            {"fact": "The project's API key is sk-abc123.", "about": "project"},
            {"fact": "The trade journal uses Node 24.", "about": "project"},
            {"fact": "Likes short answers.", "about": "user"}]}"#;
        assert_eq!(
            auto_facts(raw),
            vec![("Isaac's brother swing trades tech stocks.".to_string(), true), ("The trade journal uses Node 24.".to_string(), false)]
        );
        assert!(auto_facts("not json").is_empty());
        assert!(auto_facts(r#"{"remember": []}"#).is_empty());
        assert!(looks_secret("My password is hunter2") && looks_secret("card 4111111111111111") && looks_secret("my API key"));
        assert!(!looks_secret("Meets with Sam on 10/24 at 3pm"));
        // A changed fact finds the memory it replaces, by its words.
        let all = vec![m("1", "The user's favorite color is blue."), m("2", "The user lives in Provo.")];
        assert_eq!(closest(&all, "the user's favorite color is blue").map(|x| x.id.as_str()), Some("1"));
        assert_eq!(closest(&all, "User lives in Provo").map(|x| x.id.as_str()), Some("2"));
        assert!(closest(&all, "Owns a dog named Rex").is_none());
    }

    #[test]
    fn memories_are_encrypted_and_near_duplicates_merge() {
        let (conn, c) = setup();
        let (first, new) = add(&conn, &c, "The user teaches high school biology.", None, None).unwrap();
        assert!(new);
        let (merged, new) = add(&conn, &c, "the user teaches high school biology", None, None).unwrap();
        assert!(!new);
        assert_eq!(merged.id, first.id);
        let raw: String = conn.query_row("SELECT content FROM memories", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("biology"));
        assert_eq!(list(&conn, &c, None).len(), 1);
        assert!(add(&conn, &c, &"x".repeat(MAX_LEN + 1), None, None).is_err());
        assert!(add(&conn, &c, "   ", None, None).is_err());
    }

    #[test]
    fn project_memories_stay_in_their_project() {
        let (conn, c) = setup();
        add(&conn, &c, "Prefers metric units.", None, None).unwrap();
        add(&conn, &c, "The garden project uses raised beds.", Some("garden"), None).unwrap();
        add(&conn, &c, "The thesis is due in May.", Some("thesis"), None).unwrap();
        let garden: Vec<String> = list(&conn, &c, Some("garden")).into_iter().map(|m| m.content).collect();
        assert_eq!(garden.len(), 2);
        assert!(garden.iter().any(|m| m.contains("raised beds")) && !garden.iter().any(|m| m.contains("thesis")));
        assert_eq!(list(&conn, &c, None).len(), 3);
    }

    #[test]
    fn recall_prefers_related_memories() {
        let all = vec![
            m("1", "Has a dog named Biscuit."),
            m("2", "Works as a nurse on night shifts."),
            m("3", "Is learning Spanish."),
            m("4", "Prefers short answers."),
            m("5", "Their garden has tomatoes and basil."),
        ];
        let got = relevant(&all, "what should I plant next to my tomatoes in the garden?");
        assert_eq!(got[0].id, "5");
        // The newest few are always there for context.
        assert!(got.iter().any(|m| m.id == "1"));
    }

    /// On this PC, what it remembers goes on the newest message only: the
    /// instructions stay the same from message to message.
    #[test]
    fn recalled_memories_ride_on_the_newest_message() {
        let mem = m("1", "Has a dog named Biscuit.");
        let note = recall_note(&[&mem]);
        assert!(note.contains("not the user") && note.contains("- Has a dog named Biscuit."));
        assert_eq!(recall_note(&[]), "");
        assert!(!prompt_section(&[]).contains("Biscuit"));
        let msg = |id: &str, role: &str, text: &str| crate::db::Message { id: id.into(), role: role.into(), content: text.into(), ..Default::default() };
        let history = vec![msg("a", "user", "hi"), msg("b", "assistant", "hello"), msg("c", "user", "walk ideas?")];
        let sent = with_recall(&history, "c", &note);
        assert_eq!(sent[0].content, "hi", "earlier messages as they were");
        assert!(sent[2].content.starts_with("walk ideas?") && sent[2].content.contains("Biscuit"));
        assert_eq!(history[2].content, "walk ideas?", "what's saved is unchanged");
    }

    #[test]
    fn prompt_section_lists_memories() {
        let mem = m("1", "Has a dog named Biscuit.");
        let s = prompt_section(&[&mem]);
        assert!(s.contains("remember tool") && s.contains("- Has a dog named Biscuit."));
    }
}

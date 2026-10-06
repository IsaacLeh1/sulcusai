// SPDX-License-Identifier: AGPL-3.0-only
//! Cross-chat memory: short facts the assistant saves (or the user adds),
//! stored encrypted, and recalled into later chats when they're relevant.

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::crypto::Cipher;
use crate::db::now_ms;

pub const MAX_LEN: usize = 500;
/// Memories recalled into one chat turn.
const RECALL: usize = 8;
/// The newest memories are always included, matched or not.
const ALWAYS_RECENT: usize = 3;

#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub project_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// All memories, or those visible in one project (its own plus global ones).
pub fn list(conn: &Connection, c: &Cipher, project: Option<&str>) -> Vec<Memory> {
    let Ok(mut stmt) = conn.prepare("SELECT id, content, project_id, created_at, updated_at FROM memories ORDER BY updated_at DESC") else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(Memory {
            id: r.get(0)?,
            content: c.decrypt_or(&r.get::<_, String>(1)?, ""),
            project_id: r.get(2)?,
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
    let m = Memory { id: uuid::Uuid::new_v4().to_string(), content: content.to_string(), project_id: project_id.map(str::to_string), created_at: now, updated_at: now };
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
        "\n\n## Memory\nYou remember things across chats. When the user shares a lasting fact about themselves, their work or their \
         preferences, or asks you to remember something, save it with the remember tool, in one short sentence. Don't save \
         passwords, codes or other secrets, or details that only matter for this chat.",
    );
    if !recalled.is_empty() {
        s.push_str("\nWhat you remember from earlier chats:");
        for m in recalled {
            s.push_str(&format!("\n- {}", m.content));
        }
    }
    s
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
        Memory { id: id.into(), content: content.into(), project_id: None, created_at: 0, updated_at: 0 }
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

    #[test]
    fn prompt_section_lists_memories() {
        let mem = m("1", "Has a dog named Biscuit.");
        let s = prompt_section(&[&mem]);
        assert!(s.contains("remember tool") && s.contains("- Has a dog named Biscuit."));
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Auto-handoff: when a chat's context window is nearly full, the model
//! writes a summary and the conversation continues in a fresh chat that
//! starts from it, linked both ways.

use std::sync::atomic::AtomicBool;

use rusqlite::Connection;
use serde_json::json;

use crate::chat::{self, ContextInfo};
use crate::crypto::Cipher;
use crate::db::{self, Chat, Message};
use crate::engine::Endpoint;

/// Start a new chat once this much of the window is in use.
pub const THRESHOLD: f64 = 0.8;

pub fn needed(info: Option<&ContextInfo>) -> bool {
    info.is_some_and(|i| i.used_fraction() >= THRESHOLD)
}

const ASK: &str = "This conversation is about to continue in a fresh chat that won't see the messages above. Write a summary \
for it: the user's goals, important facts and decisions, files created or changed (with paths), and what's still left to do. \
Use short bullet points under those headings. Only include what was actually discussed.";

/// Asks the model for a handoff summary of the chat so far.
pub async fn summarize(ep: &Endpoint, base: &str, history: &[Message]) -> Result<String, String> {
    let mut history = history.to_vec();
    history.push(Message { id: uuid::Uuid::new_v4().to_string(), role: "user".into(), content: ASK.into(), ..Default::default() });
    let (kept, _) = chat::fit_history(ep, base, "", None, &history).await?;
    let done = chat::stream(ep, chat::api_messages(base, &kept), None, &AtomicBool::new(false), |_| {}).await?;
    let summary = done.content.trim().to_string();
    if summary.is_empty() {
        return Err("The model didn't write a summary.".into());
    }
    Ok(summary)
}

/// Creates the continuation chat, seeded with the summary, and leaves a
/// pointer in the old chat. Returns the new chat.
pub fn continue_in_new_chat(conn: &Connection, c: &Cipher, old: &Chat, summary: &str) -> Result<Chat, String> {
    let new = db::create_chat_in(conn, c, old.model_id.clone(), old.project_id.clone(), false)?;
    let title = format!("{} (continued)", old.title.trim_end_matches(" (continued)"));
    db::set_chat_title(conn, c, &new.id, &title)?;
    db::set_chat_mode(conn, &new.id, &old.mode)?;
    db::set_chat_web(conn, &new.id, old.web)?;
    db::set_chat_parent(conn, &new.id, &old.id)?;
    let now = db::now_ms();
    db::add_message(conn, c, &Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: new.id.clone(),
        role: "assistant".into(),
        content: format!("**Continued from “{}”.** Here's where we left off:\n\n{summary}", old.title),
        created_at: now,
        meta: Some(json!({ "handoff_from": old.id })),
        ..Default::default()
    })?;
    db::add_message(conn, c, &Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: old.id.clone(),
        role: "assistant".into(),
        content: format!("This chat got long, so it continues in a new chat: “{title}”."),
        created_at: now,
        meta: Some(json!({ "handoff_to": new.id })),
        ..Default::default()
    })?;
    Ok(db::chat(conn, c, &new.id).unwrap_or(new))
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

    #[test]
    fn handoff_triggers_at_the_threshold() {
        let mut info = ContextInfo { ctx: 1000, ..Default::default() };
        info.last_total = Some(799);
        assert!(!needed(Some(&info)));
        info.last_total = Some(800);
        assert!(needed(Some(&info)));
        assert!(!needed(None));
    }

    #[test]
    fn the_new_chat_starts_from_the_summary_and_both_link() {
        let d = std::env::temp_dir().join(format!("sulcusai-ho-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let conn = db::open(&d.join("t.db")).unwrap();
        let c = Vault::open(&d.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        let old = db::create_chat(&conn, &c, Some("m".into())).unwrap();
        db::set_chat_title(&conn, &c, &old.id, "Budget planning").unwrap();
        db::set_chat_mode(&conn, &old.id, "bypass").unwrap();
        let old = db::chat(&conn, &c, &old.id).unwrap();

        let new = continue_in_new_chat(&conn, &c, &old, "- Goal: a monthly budget").unwrap();
        assert_eq!(new.title, "Budget planning (continued)");
        assert_eq!(new.parent_id.as_deref(), Some(old.id.as_str()));
        assert_eq!(new.mode, "bypass");
        let first = &db::messages(&conn, &c, &new.id)[0];
        assert!(first.content.contains("monthly budget"));
        assert_eq!(first.meta.as_ref().unwrap()["handoff_from"], old.id);
        let note = db::messages(&conn, &c, &old.id).pop().unwrap();
        assert_eq!(note.meta.unwrap()["handoff_to"], new.id);
    }
}

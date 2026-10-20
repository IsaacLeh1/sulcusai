// SPDX-License-Identifier: AGPL-3.0-only
//! Compacting a long chat in place, the way Claude does: the model writes a
//! summary of the conversation so far, which is saved in the chat as a
//! marker. From then on the model gets that summary (in its instructions)
//! and only the messages after the marker; the person still sees everything.
//! Compacting again summarizes from the last summary on, so nothing is lost
//! between rounds.

use std::sync::atomic::AtomicBool;

use rusqlite::Connection;
use serde_json::json;

use crate::chat::{self, ContextInfo};
use crate::crypto::Cipher;
use crate::db::{self, Message};
use crate::engine::Endpoint;

const ASK: &str = "This conversation is getting long, so the messages above will be replaced by your summary of them. \
Write it so you can carry on from it alone: the user's goals, important facts, numbers and decisions, files created or changed \
(with paths), and what's still being worked on. Use short bullet points under those headings. Only include what was actually discussed.";

/// A summary marker left by compacting.
pub fn is_marker(m: &Message) -> bool {
    m.meta.as_ref().is_some_and(|v| v["compacted"] == true)
}

/// What the model sees of a chat: the messages after the last marker, and
/// that marker's summary (None when the chat was never compacted).
pub fn after_last(history: Vec<Message>) -> (Vec<Message>, Option<String>) {
    match history.iter().rposition(is_marker) {
        Some(i) => {
            let summary = history[i].content.clone();
            (history[i + 1..].to_vec(), Some(summary))
        }
        None => (history, None),
    }
}

/// The instructions section that carries the summary.
pub fn prompt_section(summary: &str) -> String {
    format!(
        "\n\n## Earlier in this conversation\nThe start of this conversation was compacted to save room. Your summary of it:\n{}",
        summary.trim()
    )
}

/// The system text the model gets: the base instructions plus `about`.
fn system(base: &str, about: &str) -> String {
    if about.trim().is_empty() { base.to_string() } else { format!("{base}\n\n{}", about.trim_start()) }
}

/// Asks the model to summarize the chat so far (from the last summary on).
pub async fn summarize(ep: &Endpoint, base: &str, history: Vec<Message>, cancel: &AtomicBool) -> Result<String, String> {
    let (mut recent, earlier) = after_last(history);
    if !recent.iter().any(|m| m.role == "user") {
        return Err("There's nothing new to compact yet.".into());
    }
    let about = earlier.as_deref().map(prompt_section).unwrap_or_default();
    recent.push(Message { id: uuid::Uuid::new_v4().to_string(), role: "user".into(), content: ASK.into(), ..Default::default() });
    let (kept, _) = chat::fit_history(ep, base, &about, None, &recent).await?;
    let done = chat::stream(ep, chat::api_messages(&system(base, &about), &kept), None, cancel, |_| {}).await?;
    if done.cancelled {
        return Err("Stopped.".into());
    }
    let summary = done.content.trim().to_string();
    if summary.is_empty() {
        return Err("The model didn't write a summary.".into());
    }
    Ok(summary)
}

/// Saves the summary as the chat's marker; `messages` is how many it covers.
pub fn save(conn: &Connection, c: &Cipher, chat_id: &str, summary: &str, messages: usize, auto: bool) -> Result<Message, String> {
    let m = Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat_id.to_string(),
        role: "assistant".into(),
        content: summary.to_string(),
        created_at: db::now_ms(),
        meta: Some(json!({ "compacted": true, "messages": messages, "auto": auto })),
        ..Default::default()
    };
    db::add_message(conn, c, &m)?;
    Ok(m)
}

/// How full the window is right after compacting (instructions and summary).
pub async fn fresh_context(ep: &Endpoint, base: &str, about: &str, summary: &str) -> Result<ContextInfo, String> {
    let about = format!("{about}{}", prompt_section(summary));
    let (_, info) = chat::fit_history(ep, base, &about, None, &[]).await?;
    Ok(info)
}

/// Messages a compaction would cover (since the last marker).
pub fn pending(history: &[Message]) -> usize {
    let start = history.iter().rposition(is_marker).map_or(0, |i| i + 1);
    history[start..].iter().filter(|m| m.role == "user" || m.role == "assistant").count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: &str, content: &str, marker: bool) -> Message {
        Message { id: content.into(), role: role.into(), content: content.into(), meta: marker.then(|| json!({ "compacted": true })), ..Default::default() }
    }

    #[test]
    fn the_model_sees_only_what_came_after_the_last_summary() {
        let history = vec![m("user", "a", false), m("assistant", "b", false), m("assistant", "first summary", true), m("user", "c", false), m("assistant", "second summary", true), m("user", "d", false), m("assistant", "e", false)];
        let (recent, summary) = after_last(history.clone());
        assert_eq!(recent.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["d", "e"]);
        assert_eq!(summary.as_deref(), Some("second summary"));
        assert_eq!(pending(&history), 2);
        let (all, none) = after_last(vec![m("user", "x", false)]);
        assert_eq!((all.len(), none), (1, None));
        assert!(prompt_section("- goal: a trip").contains("compacted"));
    }
}

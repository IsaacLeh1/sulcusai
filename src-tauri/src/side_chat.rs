// SPDX-License-Identifier: AGPL-3.0-only
//! Side questions: a quick aside about a chat ("what does that word mean?")
//! answered from the chat's recent messages, without adding anything to the
//! chat or stopping what it's doing. Answered on this PC's model, so a cloud
//! chat's messages don't go out a second time without its privacy check.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::{db, AppState, AppStateRef};

/// How much of the chat the aside sees, newest first.
const CONTEXT_CHARS: usize = 12_000;

fn messages_for(history: &[db::Message], question: &str) -> Vec<Value> {
    let mut context = String::new();
    for m in history.iter().rev().filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty()) {
        let who = if m.role == "user" { "User" } else { "Assistant" };
        let piece = format!("{who}: {}\n\n", m.content.trim());
        if context.chars().count() + piece.chars().count() > CONTEXT_CHARS {
            break;
        }
        context.insert_str(0, &piece);
    }
    vec![
        json!({ "role": "system", "content": "The user asks a quick side question about the conversation below. Answer it briefly and directly. Don't continue the conversation's task, don't use tools, and don't repeat the conversation." }),
        json!({ "role": "user", "content": format!("The conversation so far:\n\n{context}---\nSide question: {question}") }),
    ]
}

#[tauri::command]
pub async fn side_ask(state: AppStateRef<'_>, chat_id: String, question: String) -> Result<String, String> {
    let state: Arc<AppState> = state.inner().clone();
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err("Type a question.".into());
    }
    let c = state.cipher()?;
    let history = {
        let conn = state.db.lock().unwrap();
        let chat = db::chat(&conn, &c, &chat_id).ok_or("That chat is gone.")?;
        if !crate::protected::readable(&chat) {
            return Err("Unlock this chat first.".into());
        }
        db::messages(&conn, &c, &chat_id)
    };
    let (ep, _spec, _hold) = crate::turns::endpoint_for(&state, None).await?;
    if ep.cloud.is_some() {
        return Err("Side questions are answered on this PC, and no model on it is ready.".into());
    }
    let answer = crate::chat::complete(&ep, messages_for(&history, &question), json!({ "temperature": 0.3, "chat_template_kwargs": { "enable_thinking": false } }), 600).await?;
    Ok(crate::chat::strip_thinking(&answer).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a real model (SULCUSAI_TEST_GGUF): a short answer about the chat.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn e2e_a_side_question_is_answered_briefly() {
        let (_engine, ep, name) = crate::e2e::test_model().await;
        let msg = |role: &str, content: &str| db::Message { role: role.into(), content: content.into(), ..Default::default() };
        let history = [msg("user", "Plan three days in Kyoto"), msg("assistant", "Day 1: Fushimi Inari at dawn, then Gion. Day 2: Arashiyama bamboo grove. Day 3: Kinkaku-ji.")];
        let answer = crate::chat::complete(&ep, messages_for(&history, "What is Fushimi Inari?"), json!({ "temperature": 0.3, "chat_template_kwargs": { "enable_thinking": false } }), 600).await.unwrap();
        let answer = crate::chat::strip_thinking(&answer).trim().to_string();
        println!("{name}: {answer}");
        assert!(answer.to_lowercase().contains("shrine") && answer.split_whitespace().count() < 150);
    }

    #[test]
    fn the_aside_sees_the_newest_turns_and_the_question() {
        let msg = |role: &str, content: &str| db::Message { role: role.into(), content: content.into(), ..Default::default() };
        let m = messages_for(&[msg("user", "Plan a trip to Kyoto"), msg("assistant", "Day 1: Fushimi Inari"), msg("tool", "hidden")], "What is Fushimi Inari?");
        let asked = m[1]["content"].as_str().unwrap();
        assert!(asked.contains("Assistant: Day 1: Fushimi Inari") && asked.ends_with("Side question: What is Fushimi Inari?"));
        assert!(!asked.contains("hidden"));
    }
}

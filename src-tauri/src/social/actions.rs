// SPDX-License-Identifier: AGPL-3.0-only
//! Things done with one message: ask a model on this PC about it, or make a
//! reminder from it.

use std::sync::Arc;

use crate::{AppState, AppStateRef};

/// The prompt for a question about a chat message, with the talk so far.
pub fn ask_prompt(from: &str, message: &str, history: &[(String, String)], question: &str) -> String {
    let mut p = format!("Someone in a group chat, {from}, wrote this message:\n\"\"\"\n{message}\n\"\"\"\n\nAnswer the user's questions about it briefly and plainly.\n");
    for (q, a) in history {
        p.push_str(&format!("\nUser: {q}\nYou: {a}\n"));
    }
    p.push_str(&format!("\nUser: {question}\nYou:"));
    p
}

/// A question about a message, answered by a model on this PC only (the
/// message stays here).
#[tauri::command]
pub async fn social_ask(state: AppStateRef<'_>, from: String, message: String, history: Vec<(String, String)>, question: String) -> Result<String, String> {
    let state: Arc<AppState> = state.inner().clone();
    let history: Vec<(String, String)> = history.into_iter().rev().take(6).rev().collect();
    super::store::local(&state, &ask_prompt(&from, &message, &history, &question), 1200).await
}

/// A task with a reminder `minutes` from now, holding the message.
#[tauri::command]
pub async fn social_remind(state: AppStateRef<'_>, from: String, message: String, minutes: i64) -> Result<(), String> {
    let c = state.work_cipher()?;
    let first: String = message.lines().next().unwrap_or("").chars().take(80).collect();
    let mut t = crate::notes::new_task(&format!("Message from {from}: {first}"));
    let at = super::now_ms() + minutes.clamp(1, 60 * 24 * 365) * 60_000;
    t.due = Some(at);
    t.due_has_time = true;
    t.remind_at = Some(at);
    t.data.notes = message.chars().take(4000).collect();
    crate::notes::save_task(&state.db.lock().unwrap(), &c, t)?;
    Ok(())
}

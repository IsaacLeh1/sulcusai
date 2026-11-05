// SPDX-License-Identifier: AGPL-3.0-only
//! Low token mode: answers and steps that use as few tokens as still work
//! well. One switch in Settings, which a chat can turn on or off for itself.

use crate::db::{self, Message};
use rusqlite::Connection;
use serde_json::{json, Value};

/// Goes at the end of the instructions: the start stays the same with the
/// mode on or off, so the cached prefix (cloud and llama.cpp) still matches.
pub const PROMPT_BLOCK: &str = "\n\nLow token mode is on. Answer briefly: the answer first, no preamble, \
    don't restate the question or what you just did, no closing summary. Call a tool only when the answer needs it, \
    one lookup at a time, and ask for only the part of a result you need. Think only as much as the question needs.";

pub const MAX_OUT_LOCAL: u64 = 1024;
pub const MAX_OUT_CLOUD: u64 = 2048;
pub const MAX_TOOLS: usize = 24;
pub const COMPACT_AT: f64 = 0.6;
pub const MAX_TURNS: usize = 12;
/// Tool results in the latest turns keep this much; older ones less.
pub const RESULT_CHARS: usize = 6000;
pub const OLD_RESULT_CHARS: usize = 1500;
const RECENT_TURNS: usize = 3;

fn chat_key(chat_id: &str) -> String {
    format!("low_tokens:{chat_id}")
}

pub fn global(conn: &Connection) -> bool {
    db::get::<bool>(conn, "low_tokens").unwrap_or(false)
}

/// The chat's own choice (None: it follows Settings).
pub fn chat_choice(conn: &Connection, chat_id: &str) -> Option<bool> {
    db::get::<Option<bool>>(conn, &chat_key(chat_id)).flatten()
}

pub fn decide(global: bool, chat: Option<bool>) -> bool {
    chat.unwrap_or(global)
}

pub fn is_on(conn: &Connection, chat_id: &str) -> bool {
    decide(global(conn), chat_choice(conn, chat_id))
}

/// Smaller output and thinking. A lower limit already set stays.
pub fn tune_extra(extra: &mut Value, cloud: bool) {
    if !extra.is_object() {
        *extra = json!({});
    }
    let cap = if cloud { MAX_OUT_CLOUD } else { MAX_OUT_LOCAL };
    let now = extra.get("max_tokens").and_then(Value::as_u64).unwrap_or(u64::MAX);
    extra["max_tokens"] = json!(now.min(cap));
    extra["reasoning_effort"] = json!("low");
    if let Some(k) = extra.get_mut("chat_template_kwargs").and_then(Value::as_object_mut) {
        k.insert("reasoning_effort".into(), json!("low"));
    }
}

/// The first `MAX_TOOLS` tools, in the order offered: the always-needed ones come first.
pub fn fewer_tools(tools: &mut Option<Value>) {
    if let Some(a) = tools.as_mut().and_then(Value::as_array_mut) {
        a.truncate(MAX_TOOLS);
    }
}

pub fn compact_at(at: f64) -> f64 {
    at.min(COMPACT_AT)
}

/// Keeps the middle out of a long result, saying so, so the model can ask for that part.
pub fn cut_result(text: &str, keep: usize) -> String {
    if text.chars().count() <= keep {
        return text.to_string();
    }
    let head: String = text.chars().take(keep * 2 / 3).collect();
    let tail_n = keep / 3;
    let total = text.chars().count();
    let tail: String = text.chars().skip(total - tail_n).collect();
    format!(
        "{head}\n[Cut off: {} of {total} characters left out in low token mode. Ask the tool for that part (a page, offset or narrower query) if you need it.]\n{tail}",
        total - head.chars().count() - tail_n
    )
}

/// The last `MAX_TURNS` turns (a turn starts at a user message), with tool
/// results cut: hardest in the older turns.
pub fn window(history: &[Message]) -> Vec<Message> {
    let starts: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == "user").map(|(i, _)| i).collect();
    let from = if starts.len() > MAX_TURNS { starts[starts.len() - MAX_TURNS] } else { 0 };
    let recent_from = if starts.len() > RECENT_TURNS { starts[starts.len() - RECENT_TURNS] } else { 0 };
    history[from..]
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut m = m.clone();
            if m.role == "tool" {
                let keep = if from + i >= recent_from { RESULT_CHARS } else { OLD_RESULT_CHARS };
                m.content = cut_result(&m.content, keep);
            }
            m
        })
        .collect()
}

/// Marks a reply as made in low token mode, for the savings shown in the chat.
pub fn mark(meta: &mut Value) {
    if let Some(o) = meta.as_object_mut() {
        o.insert("low_tokens".into(), json!(true));
    }
}

#[derive(serde::Serialize)]
pub struct LowTokens {
    pub global: bool,
    pub chat: Option<bool>,
    pub on: bool,
}

#[tauri::command]
pub fn low_tokens_get(state: crate::AppStateRef<'_>, chat_id: Option<String>) -> LowTokens {
    let conn = state.db.lock().unwrap();
    let g = global(&conn);
    let c = chat_id.as_deref().and_then(|id| chat_choice(&conn, id));
    LowTokens { global: g, chat: c, on: decide(g, c) }
}

#[tauri::command]
pub fn low_tokens_set(state: crate::AppStateRef<'_>, chat_id: Option<String>, on: Option<bool>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    match chat_id {
        Some(id) => db::set(&conn, &chat_key(&id), &on),
        None => db::set(&conn, "low_tokens", &on.unwrap_or(false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> Message {
        Message { role: role.into(), content: content.into(), ..Default::default() }
    }

    #[test]
    fn chat_choice_wins() {
        assert!(!decide(false, None));
        assert!(decide(true, None));
        assert!(decide(false, Some(true)));
        assert!(!decide(true, Some(false)));
    }

    #[test]
    fn output_capped_but_lower_kept() {
        let mut e = json!({});
        tune_extra(&mut e, false);
        assert_eq!(e["max_tokens"], 1024);
        assert_eq!(e["reasoning_effort"], "low");
        let mut e = json!({ "max_tokens": 300, "chat_template_kwargs": { "enable_thinking": true } });
        tune_extra(&mut e, true);
        assert_eq!(e["max_tokens"], 300);
        assert_eq!(e["chat_template_kwargs"]["reasoning_effort"], "low");
    }

    #[test]
    fn tools_and_compaction() {
        let mut t = Some(Value::Array((0..40).map(|i| json!(i)).collect()));
        fewer_tools(&mut t);
        assert_eq!(t.unwrap().as_array().unwrap().len(), MAX_TOOLS);
        assert_eq!(compact_at(0.8), COMPACT_AT);
        assert_eq!(compact_at(0.5), 0.5);
    }

    #[test]
    fn long_result_cut_with_note() {
        let text = "a".repeat(10_000);
        let cut = cut_result(&text, 600);
        assert!(cut.len() < 1000);
        assert!(cut.contains("Cut off: 9400 of 10000"));
        assert_eq!(cut_result("short", 600), "short");
    }

    #[test]
    fn window_keeps_last_turns_and_cuts_old_results() {
        let mut h = Vec::new();
        for _ in 0..20 {
            h.push(msg("user", "q"));
            h.push(msg("tool", &"x".repeat(5000)));
        }
        let w = window(&h);
        assert_eq!(w.iter().filter(|m| m.role == "user").count(), MAX_TURNS);
        assert!(w[1].content.contains("Cut off"));
        assert_eq!(w.last().unwrap().content.len(), 5000);
    }

    #[test]
    fn prompt_block_is_appended_not_prefixed() {
        assert!(PROMPT_BLOCK.starts_with("\n\n"));
        let mut m = json!({});
        mark(&mut m);
        assert_eq!(m["low_tokens"], true);
    }
}

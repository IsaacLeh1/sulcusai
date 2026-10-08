// SPDX-License-Identifier: AGPL-3.0-only
//! Builds the prompt (profile + history within the context window) and
//! streams the model's reply, including any tool calls it makes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};

use crate::db::{Message, Profile, ToolCall};
use crate::engine::Endpoint;
use crate::net;

/// Tokens each message adds for its role markers in the chat template.
const PER_MESSAGE_OVERHEAD: u32 = 6;
const TRIMMED_TOOL_OUTPUT: &str = "[Earlier tool output removed to make room. Run the tool again if you need it.]";

pub fn system_prompt(profile: &Profile, today: &str) -> (String, String) {
    let base = format!(
        "You are SulcusAI, a helpful assistant that runs privately on the user's own computer. \
         Nothing in this conversation leaves their PC. Be clear, accurate and friendly. \
         Use Markdown when it helps. If you don't know something, say so.\n\nToday's date is {today}."
    );
    let mut about = String::new();
    if !profile.name.trim().is_empty() {
        about.push_str(&format!("The user's name is {}.\n", profile.name.trim()));
    }
    if !profile.about.trim().is_empty() {
        about.push_str(&format!("About the user, in their words:\n{}\n", profile.about.trim()));
    }
    if !profile.preferences.trim().is_empty() {
        about.push_str(&format!("How the user wants you to respond:\n{}\n", profile.preferences.trim()));
    }
    (base, about)
}

/// Context window usage for the meter.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ContextInfo {
    pub ctx: u32,
    pub system_tokens: u32,
    pub profile_tokens: u32,
    pub tools_tokens: u32,
    pub history_tokens: u32,
    pub reply_reserve: u32,
    pub dropped_messages: usize,
    /// Prompt + reply tokens of the last finished step, as the engine counted them.
    pub last_total: Option<u32>,
}

impl ContextInfo {
    /// Share of the window in use, 0.0–1.0.
    pub fn used_fraction(&self) -> f64 {
        let used = self.last_total.unwrap_or(self.system_tokens + self.profile_tokens + self.tools_tokens + self.history_tokens);
        used as f64 / self.ctx.max(1) as f64
    }
}

/// Token counts are cached per engine run (the port changes when the model does).
fn cache() -> &'static Mutex<HashMap<String, u32>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn count_tokens(ep: &Endpoint, text: &str) -> Result<u32, String> {
    if text.is_empty() {
        return Ok(0);
    }
    let resp: Value = net::local_client()
        .post(ep.url("/tokenize"))
        .bearer_auth(&ep.key)
        .json(&json!({ "content": text }))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp["tokens"].as_array().map_or(0, |t| t.len() as u32))
}

async fn message_tokens(ep: &Endpoint, m: &Message) -> Result<u32, String> {
    let key = format!("{}:{}", ep.port, m.id);
    if let Some(n) = cache().lock().unwrap().get(&key) {
        return Ok(*n);
    }
    let mut text = m.content.clone();
    for c in m.tool_calls.iter().flatten() {
        text.push_str(&c.name);
        text.push_str(&c.arguments);
    }
    let n = count_tokens(ep, &text).await? + PER_MESSAGE_OVERHEAD;
    let mut c = cache().lock().unwrap();
    if c.len() > 20_000 {
        c.clear();
    }
    c.insert(key, n);
    Ok(n)
}

/// Splits history into turns, each starting at a user message, so a tool
/// call is never kept without its result (or the other way round).
fn turns(history: &[Message]) -> Vec<&[Message]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, m) in history.iter().enumerate() {
        if m.role == "user" && i > start {
            out.push(&history[start..i]);
            start = i;
        }
    }
    if start < history.len() {
        out.push(&history[start..]);
    }
    out
}

/// Keeps the newest turns that fit. If even the current turn is too big,
/// older tool outputs inside it are replaced with a short note.
pub async fn fit_history(
    ep: &Endpoint,
    base: &str,
    about: &str,
    tools: Option<&Value>,
    history: &[Message],
) -> Result<(Vec<Message>, ContextInfo), String> {
    let system_tokens = count_tokens(ep, base).await? + PER_MESSAGE_OVERHEAD;
    let profile_tokens = count_tokens(ep, about).await?;
    let tools_tokens = match tools {
        Some(t) => count_tokens(ep, &t.to_string()).await?,
        None => 0,
    };
    let reply_reserve = (ep.ctx / 4).clamp(256, 4096);
    let budget = ep.ctx.saturating_sub(system_tokens + profile_tokens + tools_tokens + reply_reserve);

    let groups = turns(history);
    let mut kept: Vec<Vec<Message>> = Vec::new();
    let mut used = 0u32;
    for (gi, group) in groups.iter().enumerate().rev() {
        let mut sizes = Vec::with_capacity(group.len());
        for m in group.iter() {
            sizes.push(message_tokens(ep, m).await?);
        }
        let total: u32 = sizes.iter().sum();
        let newest = gi == groups.len() - 1;
        if used + total <= budget {
            used += total;
            kept.push(group.to_vec());
            continue;
        }
        if newest {
            // Shrink the current turn's oldest tool outputs until it fits.
            let mut group = group.to_vec();
            let mut total = total;
            let stub = count_tokens(ep, TRIMMED_TOOL_OUTPUT).await? + PER_MESSAGE_OVERHEAD;
            for (m, size) in group.iter_mut().zip(sizes.iter()) {
                if total <= budget {
                    break;
                }
                if m.role == "tool" && *size > stub {
                    m.content = TRIMMED_TOOL_OUTPUT.into();
                    total = total - size + stub;
                }
            }
            used += total;
            kept.push(group);
        }
        break;
    }
    kept.reverse();
    let kept: Vec<Message> = kept.into_iter().flatten().collect();
    let info = ContextInfo {
        ctx: ep.ctx,
        system_tokens,
        profile_tokens,
        tools_tokens,
        history_tokens: used,
        reply_reserve,
        dropped_messages: history.len().saturating_sub(kept.len()),
        last_total: None,
    };
    Ok((kept, info))
}

/// History in the OpenAI chat format the engine expects.
pub fn api_messages(system: &str, history: &[Message]) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": system })];
    for m in history {
        match m.role.as_str() {
            "tool" => out.push(json!({ "role": "tool", "tool_call_id": m.tool_call_id, "content": m.content })),
            "assistant" if m.tool_calls.as_ref().is_some_and(|t| !t.is_empty()) => {
                let calls: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .flatten()
                    .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } }))
                    .collect();
                out.push(json!({ "role": "assistant", "content": m.content, "tool_calls": calls }));
            }
            role => out.push(json!({ "role": role, "content": m.content })),
        }
    }
    out
}

pub enum Delta {
    Content(String),
    Thinking(String),
}

pub struct Finished {
    pub content: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub tps: Option<f64>,
    pub cancelled: bool,
}

/// Streams one model reply. `on_delta` gets text as it arrives.
pub async fn stream(
    ep: &Endpoint,
    messages: Vec<Value>,
    tools: Option<&Value>,
    cancel: &AtomicBool,
    mut on_delta: impl FnMut(Delta),
) -> Result<Finished, String> {
    let mut body = json!({
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
        "temperature": 0.6,
    });
    if let Some(t) = tools.filter(|t| t.as_array().is_some_and(|a| !a.is_empty())) {
        body["tools"] = t.clone();
    }

    let resp = net::local_client()
        .post(ep.url("/v1/chat/completions"))
        .bearer_auth(&ep.key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the engine: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.pointer("/error/message").and_then(|m| m.as_str()).map(str::to_string))
            .unwrap_or(text);
        if detail.contains("exceed") && detail.contains("context") {
            return Err("This conversation no longer fits in the model's memory. Start a new chat to continue.".into());
        }
        return Err(format!("The engine returned {status}: {detail}"));
    }

    let mut out = Finished {
        content: String::new(),
        thinking: String::new(),
        tool_calls: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
        tps: None,
        cancelled: false,
    };
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    'read: while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        let chunk = chunk.map_err(|e| format!("The reply was interrupted: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line: String = buf.drain(..=nl).collect();
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break 'read;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(delta) = v.pointer("/choices/0/delta") {
                if let Some(t) = delta.get("reasoning_content").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                    out.thinking.push_str(t);
                    on_delta(Delta::Thinking(t.to_string()));
                }
                if let Some(t) = delta.get("content").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                    out.content.push_str(t);
                    on_delta(Delta::Content(t.to_string()));
                }
                for tc in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    merge_tool_delta(&mut out.tool_calls, tc);
                }
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                out.prompt_tokens = u["prompt_tokens"].as_u64().map(|n| n as u32);
                out.completion_tokens = u["completion_tokens"].as_u64().map(|n| n as u32);
            }
            if let Some(t) = v.pointer("/timings/predicted_per_second").and_then(|t| t.as_f64()) {
                out.tps = Some((t * 10.0).round() / 10.0);
            }
        }
    }
    // Small models sometimes write a tool call into their thinking or their
    // text instead of the tool-call channel; run it rather than lose it.
    if out.tool_calls.is_empty() && !out.cancelled {
        let offered: Vec<String> = tools.and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str).map(str::to_string)).collect();
        if !offered.is_empty() {
            let from_text = salvage_tool_calls(&out.content, &offered);
            if !from_text.is_empty() {
                out.content = TOOL_CALL_BLOCK.replace_all(&out.content, "").trim().to_string();
                out.tool_calls = from_text;
            } else if out.content.trim().is_empty() {
                out.tool_calls = salvage_tool_calls(&out.thinking, &offered);
            }
        }
    }
    for (i, c) in out.tool_calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{}_{i}", uuid::Uuid::new_v4().simple());
        }
    }
    out.tool_calls.retain(|c| !c.name.is_empty());
    Ok(out)
}

static TOOL_CALL_BLOCK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)<tool_call>\s*(\{.*?\})\s*</tool_call>").unwrap());

/// `<tool_call>{"name": ..., "arguments": {...}}</tool_call>` blocks in text,
/// for tools that were offered.
fn salvage_tool_calls(text: &str, offered: &[String]) -> Vec<ToolCall> {
    TOOL_CALL_BLOCK
        .captures_iter(text)
        .filter_map(|c| {
            let v: Value = serde_json::from_str(&c[1]).ok()?;
            let name = v["name"].as_str()?.to_string();
            if !offered.contains(&name) {
                return None;
            }
            let arguments = match &v["arguments"] {
                Value::String(s) => s.clone(),
                Value::Null => "{}".into(),
                other => other.to_string(),
            };
            Some(ToolCall { id: String::new(), name, arguments })
        })
        .collect()
}

/// One reply, not streamed, for background work such as meeting notes and
/// translation. `extra` is merged into the request (e.g. response_format).
pub async fn complete(ep: &Endpoint, messages: Vec<Value>, extra: Value, max_tokens: u32) -> Result<String, String> {
    let mut body = json!({
        "messages": messages,
        "temperature": 0.3,
        "max_tokens": max_tokens,
        // Background work doesn't need step-by-step thinking (models
        // without this switch ignore it).
        "chat_template_kwargs": { "enable_thinking": false },
    });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    let resp = net::local_client()
        .post(ep.url("/v1/chat/completions"))
        .bearer_auth(&ep.key)
        .json(&body)
        .timeout(Duration::from_secs(900))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the engine: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| format!("The engine sent an unreadable answer: {e}"))?;
    if !status.is_success() {
        let detail = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
        return Err(format!("The engine returned {status}: {detail}"));
    }
    let content = v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or_default();
    Ok(strip_thinking(content).trim().to_string())
}

/// Removes a <think>…</think> block some models write before the answer.
pub fn strip_thinking(s: &str) -> &str {
    match s.find("</think>") {
        Some(i) if s.trim_start().starts_with("<think>") => &s[i + "</think>".len()..],
        _ => s,
    }
}

/// Tool calls arrive in pieces: the first piece names the tool, later
/// pieces append to its arguments. `index` says which call a piece is for.
fn merge_tool_delta(calls: &mut Vec<ToolCall>, tc: &Value) {
    let index = tc.get("index").and_then(Value::as_u64).unwrap_or(calls.len().saturating_sub(1) as u64) as usize;
    while calls.len() <= index {
        calls.push(ToolCall { id: String::new(), name: String::new(), arguments: String::new() });
    }
    let call = &mut calls[index];
    if let Some(id) = tc.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        call.id = id.to_string();
    }
    if let Some(f) = tc.get("function") {
        if let Some(n) = f.get("name").and_then(Value::as_str) {
            call.name.push_str(n);
        }
        if let Some(a) = f.get("arguments").and_then(Value::as_str) {
            call.arguments.push_str(a);
        }
    }
}

/// A short sidebar title from the first message.
/// A short title (2-6 words) summarizing a chat's first request, written by
/// the model. None if it gives nothing usable (the first line stays).
pub async fn summary_title(ep: &Endpoint, request: &str) -> Option<String> {
    let request: String = request.chars().take(2000).collect();
    let messages = vec![
        json!({ "role": "system", "content": "You name chats. Reply with a short title of 2 to 6 words that sums up what the user wants. Title only: no quotes, no ending period, no emoji." }),
        json!({ "role": "user", "content": request }),
    ];
    let raw = complete(ep, messages, json!({}), 24).await.ok()?;
    clean_title(&raw)
}

fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_start_matches(|c: char| c == '#' || c == '*' || c.is_whitespace());
    let line = line.strip_prefix("Title:").unwrap_or(line).trim();
    let t: String = line.trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '*' | '.' | '`')).trim().chars().take(60).collect();
    (!t.is_empty() && t.split_whitespace().count() <= 10).then_some(t)
}

pub fn title_from(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("New chat");
    let mut title: String = line.chars().take(48).collect();
    if line.chars().count() > 48 {
        title = title.trim_end().to_string() + "…";
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: &str, id: &str) -> Message {
        Message { id: id.into(), role: role.into(), content: id.into(), ..Default::default() }
    }

    #[test]
    fn title_uses_first_nonempty_line_and_truncates() {
        assert_eq!(title_from("\n  hello there \nmore"), "hello there");
        let offered = vec!["web_search".to_string()];
        let calls = salvage_tool_calls("\n<tool_call>\n{\"name\": \"web_search\", \"arguments\": {\"query\": \"Mount Timpanogos height\"}}\n</tool_call>", &offered);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web_search");
        assert!(calls[0].arguments.contains("Timpanogos"));
        assert!(salvage_tool_calls("<tool_call>{\"name\": \"delete_everything\", \"arguments\": {}}</tool_call>", &offered).is_empty(), "only offered tools");
        assert_eq!(clean_title("\"Trip to Tokyo.\"").as_deref(), Some("Trip to Tokyo"));
        assert_eq!(clean_title("Title: Budget plan\nextra").as_deref(), Some("Budget plan"));
        assert_eq!(clean_title("   "), None);
        let long = "a".repeat(60);
        let t = title_from(&long);
        assert!(t.ends_with('…'));
        assert_eq!(t.chars().count(), 49);
    }

    #[test]
    fn profile_section_is_empty_when_profile_is_blank() {
        let (base, about) = system_prompt(&Profile::default(), "2026-10-06");
        assert!(base.contains("2026-10-06"));
        assert!(about.is_empty());
    }

    #[test]
    fn profile_section_includes_what_the_user_wrote() {
        let p = Profile { name: "Sam".into(), about: "I teach biology.".into(), preferences: "Be brief.".into() };
        let (_, about) = system_prompt(&p, "2026-10-06");
        assert!(about.contains("Sam") && about.contains("biology") && about.contains("Be brief."));
    }

    #[test]
    fn turns_start_at_user_messages_and_keep_tool_pairs_together() {
        let h = vec![m("user", "u1"), m("assistant", "a1"), m("tool", "t1"), m("assistant", "a2"), m("user", "u2"), m("assistant", "a3")];
        let t = turns(&h);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].len(), 4);
        assert_eq!(t[1][0].id, "u2");
    }

    #[test]
    fn tool_call_pieces_are_merged_by_index() {
        let mut calls = Vec::new();
        merge_tool_delta(&mut calls, &json!({ "index": 0, "id": "c1", "function": { "name": "read_file", "arguments": "" } }));
        merge_tool_delta(&mut calls, &json!({ "index": 0, "function": { "arguments": "{\"path\":" } }));
        merge_tool_delta(&mut calls, &json!({ "index": 1, "id": "c2", "function": { "name": "list_dir", "arguments": "{}" } }));
        merge_tool_delta(&mut calls, &json!({ "index": 0, "function": { "arguments": "\"a.txt\"}" } }));
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, r#"{"path":"a.txt"}"#);
        assert_eq!(calls[1].name, "list_dir");
    }

    #[test]
    fn api_messages_use_openai_tool_shapes() {
        let mut a = m("assistant", "a1");
        a.tool_calls = Some(vec![ToolCall { id: "c1".into(), name: "list_dir".into(), arguments: "{}".into() }]);
        let mut t = m("tool", "t1");
        t.tool_call_id = Some("c1".into());
        let msgs = api_messages("sys", &[m("user", "u1"), a, t]);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "list_dir");
        assert_eq!(msgs[3]["tool_call_id"], "c1");
    }
}

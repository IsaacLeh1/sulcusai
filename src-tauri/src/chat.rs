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
         Use Markdown when it helps. If you don't know something, say so.\n\n\
         When a question needs facts you don't have, get them yourself with your tools (weather, web search, \
         notes, tasks, calendar, email, files) instead of asking the user. Ask only for what you can't look up, \
         and never guess figures.\n\nToday's date is {today}."
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
    if !profile.location.trim().is_empty() {
        about.push_str(&format!("The user is in {}. Use this for weather and anything local unless they name another place.\n", profile.location.trim()));
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
    if ep.cloud.is_some() {
        return Ok(crate::cloud::estimate_tokens(text));
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
            // When each message was sent, so "this afternoon" or "tonight"
            // make sense. Fixed per message, so the engine's cache holds.
            "user" if m.created_at > 0 => out.push(json!({ "role": "user", "content": format!("{}\n{}", sent_note(m.created_at), m.content) })),
            role => out.push(json!({ "role": role, "content": m.content })),
        }
        if let Some(blocks) = m.meta.as_ref().and_then(|v| v.get("cloud_blocks")) {
            if let Some(last) = out.last_mut() {
                last["cloud_blocks"] = blocks.clone();
            }
        }
    }
    out
}

/// Puts attached pictures into the user messages of `out` (as built by
/// `api_messages`: the system message, then one per history message).
/// Only the newest few are sent, which keeps long chats affordable. With no
/// `see`, the model is told it can't see them, so it can say so.
pub fn attach_images(out: &mut [Value], history: &[Message], see: Option<&dyn Fn(&str) -> Option<String>>) {
    let mut left = 4usize;
    for (i, m) in history.iter().enumerate().rev() {
        if m.role != "user" {
            continue;
        }
        let Some(ids) = m.meta.as_ref().and_then(|v| v.get("images")).and_then(Value::as_array) else { continue };
        let ids: Vec<&str> = ids.iter().filter_map(Value::as_str).collect();
        let Some(slot) = out.get_mut(i + 1) else { continue };
        let text = slot["content"].as_str().unwrap_or("").to_string();
        let n = ids.len();
        slot["content"] = match see {
            Some(see) if left > 0 => {
                let mut parts = vec![json!({ "type": "text", "text": text })];
                for id in ids {
                    if left == 0 {
                        break;
                    }
                    if let Some(url) = see(id) {
                        parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
                        left -= 1;
                    }
                }
                Value::Array(parts)
            }
            Some(_) => json!(format!("{text}\n[{n} picture(s) were attached here earlier.]")),
            None => json!(format!(
                "{text}\n[The user attached {n} picture(s), but the model in use can't see pictures. Tell them, and suggest picking a model that can, \
                 such as Qwen3 VL or Gemma 3, from the model menu.]"
            )),
        };
    }
}

fn sent_note(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .earliest()
        .map(|t| format!("(Sent {})", t.format("%a %b %-d %Y, %-I:%M %p, UTC%:z")))
        .unwrap_or_default()
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
    /// The reply's content blocks as a cloud provider sent them, kept so its
    /// reasoning can be passed back during a run of tool calls.
    pub raw: Option<Value>,
}

/// Streams one model reply. `on_delta` gets text as it arrives.
pub async fn stream(
    ep: &Endpoint,
    messages: Vec<Value>,
    tools: Option<&Value>,
    cancel: &AtomicBool,
    mut on_delta: impl FnMut(Delta),
) -> Result<Finished, String> {
    if let Some(t) = &ep.cloud {
        return crate::cloud::stream(t, &ep.extra, messages, tools, cancel, on_delta).await;
    }
    // Saved cloud reply blocks are for cloud providers only.
    let messages: Vec<Value> = messages
        .into_iter()
        .map(|mut m| {
            if let Some(o) = m.as_object_mut() {
                o.remove("cloud_blocks");
            }
            m
        })
        .collect();
    let mut body = json!({
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
        "temperature": 0.6,
        // One step's reply: small models that start rambling would otherwise
        // write until the context is full (minutes). Advanced settings can override.
        "max_tokens": (ep.ctx / 2).clamp(512, 4096),
    });
    if let Some(t) = tools.filter(|t| t.as_array().is_some_and(|a| !a.is_empty())) {
        body["tools"] = t.clone();
    }
    if let (Some(b), Some(e)) = (body.as_object_mut(), ep.extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
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
        raw: None,
    };
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    let offered = offered_names(tools);
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
                    // A tool call written as text is complete: stop here. Left
                    // going, small models imagine its result and keep writing
                    // (minutes of text that would be thrown away). Dropping the
                    // stream stops the engine.
                    if t.contains('}') && out.tool_calls.is_empty() && !offered.is_empty() && !salvage(&out.content, &offered).0.is_empty() {
                        break 'read;
                    }
                    // Stuck saying the same line over and over: stop it there.
                    if t.contains('\n') && repeats_itself(&out.content) {
                        break 'read;
                    }
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
    rescue_tool_calls(&mut out, tools);
    for (i, c) in out.tool_calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{}_{i}", uuid::Uuid::new_v4().simple());
        }
    }
    out.tool_calls.retain(|c| !c.name.is_empty());
    Ok(out)
}

/// The last few finished lines are the same line (a small model stuck in a loop).
fn repeats_itself(text: &str) -> bool {
    let done = match text.rfind('\n') {
        Some(i) => &text[..i],
        None => return false,
    };
    let lines: Vec<&str> = done.lines().map(str::trim).filter(|l| !l.is_empty()).rev().take(4).collect();
    lines.len() == 4 && lines[0].len() > 8 && lines.iter().all(|l| *l == lines[0])
}

fn offered_names(tools: Option<&Value>) -> Vec<String> {
    tools.and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str).map(str::to_string)).collect()
}

/// Small models sometimes write a tool call into their thinking or their
/// text instead of the tool-call channel; run it rather than lose it.
pub(crate) fn rescue_tool_calls(out: &mut Finished, tools: Option<&Value>) {
    if out.tool_calls.is_empty() && !out.cancelled {
        let offered = offered_names(tools);
        if !offered.is_empty() {
            let (calls, rest) = salvage(&out.content, &offered);
            if !calls.is_empty() {
                out.content = rest;
                out.tool_calls = calls;
            } else if out.content.trim().is_empty() {
                out.tool_calls = salvage(&out.thinking, &offered).0;
            }
        }
    }
}

static ABOUT_THEIR_THINGS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(e-?mails?|mail|inbox|messages?|calendar|events?|schedule|meetings?|appointments?|party|birthday|today|tonight|tomorrow|this week|next week|notes?|tasks?|to-?dos?|reminders?|weather|forecast)\b").unwrap()
});
// "I don't have access to your messages": wrong whenever tools were offered,
// whatever the question.
static NO_ACCESS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)(don'?t|do not|can'?t|cannot) (have )?(direct |any |real-time |personal )?access|(unable|not able) to (access|read|check|see)|i (can'?t|cannot) (read|check|see) your|i don'?t have (any )?(information|details) about|without you providing|i don'?t have any (email|calendar|accounts?)").unwrap()
});
static STALLING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)i'?ll need to|i need to|i will need|i would need|please (provide|confirm|specify)|(could|can) you (please )?(provide|confirm|specify|tell me|share|give me)|(any|more|specific) (details|criteria|information)|function call|<tool|(i'?ll|i will|let me) (search|look|check)|(don'?t|do not|can'?t|cannot) (have )?access|(unable|not able) to (access|read|check|see)|i (can'?t|cannot) (read|check|see)").unwrap()
});

/// A small model asked the user for details or permission about their own
/// mail, calendar, notes, tasks or weather instead of using a tool to look.
pub fn stalls(user: &str, reply: &str, offered: &[String]) -> bool {
    let reply = reply.trim().replace(['\u{2019}', '\u{2018}'], "'");
    if reply.is_empty() {
        return false;
    }
    // Telling the user to "use email_search" instead of using it.
    if offered.iter().any(|n| n.contains('_') && reply.contains(n.as_str())) {
        return true;
    }
    // Saying it has no access is wrong however long the reply goes on.
    let opening: String = reply.chars().take(600).collect();
    NO_ACCESS.is_match(&opening) || (reply.chars().count() < 800 && ABOUT_THEIR_THINGS.is_match(user) && STALLING.is_match(&reply))
}

/// For a model that stalled: its look-up tools (most relevant to the question
/// first), what to tell it, and a JSON schema that makes the engine accept
/// nothing but one call to one of them. (`tool_choice: "required"` isn't
/// enforced by llama.cpp for every model; a response schema always is.)
pub struct ForcedLookup {
    pub names: Vec<String>,
    pub nudge: String,
    pub response_format: Value,
}

pub fn forced_lookup(tools: &Value, asked: &str, looks_only: impl Fn(&str) -> bool) -> Option<ForcedLookup> {
    let q = asked.to_lowercase();
    let first: &[(&str, &[&str])] = &[
        ("calendar_events", &["calendar", "event", "party", "meeting", "appointment", "schedule", "when is", "today", "tomorrow", "birthday"]),
        ("email_search", &["email", "mail", "inbox", "message", "sent", "ask", "wrote", "said", "reply"]),
        ("search_notes", &["note"]),
        ("list_tasks", &["task", "to-do", "todo", "remind"]),
        ("weather", &["weather", "forecast", "rain", "temperature"]),
        ("search_meetings", &["meeting notes", "transcript"]),
    ];
    let rank = |name: &str| first.iter().position(|(n, words)| *n == name && words.iter().any(|w| q.contains(w))).unwrap_or(usize::MAX);
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let mut list: Vec<&Value> = tools.as_array()?.iter().filter(|t| looks_only(&name_of(t))).collect();
    if list.is_empty() {
        return None;
    }
    list.sort_by_key(|t| rank(&name_of(t)));
    // Some tools fit the question: offer only those, so it can't wander off.
    if rank(&name_of(list[0])) != usize::MAX {
        list.retain(|t| rank(&name_of(t)) != usize::MAX);
    }
    let names: Vec<String> = list.iter().map(|t| name_of(t)).collect();
    let listing: Vec<String> = list.iter().map(|t| format!("- {}: {}", name_of(t), t["function"]["description"].as_str().unwrap_or(""))).collect();
    let schema = json!({ "oneOf": list.iter().map(|t| json!({
        "type": "object",
        "properties": { "name": { "const": name_of(t) }, "arguments": t["function"]["parameters"] },
        "required": ["name", "arguments"],
    })).collect::<Vec<_>>() });
    Some(ForcedLookup {
        names,
        nudge: format!(
            "(Don't ask me for details or confirmation, and don't say you can't access it: look it up now. Reply with only one tool call as JSON, {{\"name\": ..., \"arguments\": {{...}}}}, using one of these tools:\n{})",
            listing.join("\n")
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// The call a forced lookup replied with.
pub fn forced_call(content: &str, names: &[String]) -> Option<ToolCall> {
    let v: Value = serde_json::from_str(content.trim()).ok()?;
    call_from_value(&v, names)
}

// What's left around a call once its JSON is taken out: the tags models wrap
// calls in (<tool_call>, Qwen's own; <tools>, which coder models copy from
// their template; <tool>, <function_call>) or an empty ```json fence.
static EMPTY_WRAPPER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?s)<(?:tool_call|tools?|function_call)>\s*</(?:tool_call|tools?|function_call)>|```(?:json|tool_call)?\s*```").unwrap()
});

/// The first tool call written into text, for tools that were offered:
/// `{"name": ..., "arguments": {...}}` anywhere in it, wrapped in a tag or a
/// code fence or not. Returns it and the words before it. Only the first:
/// what a model writes after a call it can't see the result of is made up
/// (it "imagines" the result and goes on calling tools with invented details).
fn salvage(text: &str, offered: &[String]) -> (Vec<ToolCall>, String) {
    let mut i = 0;
    while let Some(off) = text[i..].find('{') {
        let start = i + off;
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        if let Some(Ok(v)) = stream.next() {
            if let Some(call) = call_from_value(&v, offered) {
                let before = EMPTY_WRAPPER.replace_all(&text[..start], "").to_string();
                let before = before.trim_end().trim_end_matches(|c| c == '<' || c == '`').trim_end_matches("<tool_call>").trim_end_matches("<tools>").trim_end_matches("```json").trim();
                return (vec![call], before.to_string());
            }
        }
        i = start + 1;
    }
    (Vec::new(), text.trim().to_string())
}

#[cfg(test)]
fn salvage_tool_calls(text: &str, offered: &[String]) -> Vec<ToolCall> {
    salvage(text, offered).0
}

fn call_from_value(v: &Value, offered: &[String]) -> Option<ToolCall> {
    let name = v["name"].as_str()?.to_string();
    if !offered.contains(&name) {
        return None;
    }
    let args = if v["arguments"].is_null() { &v["parameters"] } else { &v["arguments"] };
    let arguments = match args {
        Value::String(s) => s.clone(),
        Value::Null => "{}".into(),
        other => other.to_string(),
    };
    Some(ToolCall { id: String::new(), name, arguments })
}

/// One reply, not streamed, for background work such as meeting notes and
/// translation. `extra` is merged into the request (e.g. response_format).
pub async fn complete(ep: &Endpoint, messages: Vec<Value>, extra: Value, max_tokens: u32) -> Result<String, String> {
    if let Some(t) = &ep.cloud {
        return crate::cloud::complete(t, messages).await;
    }
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
pub(crate) fn merge_tool_delta(calls: &mut Vec<ToolCall>, tc: &Value) {
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
        // How qwen2.5-coder wrote one: in the <tools> tag from its template.
        let cal = vec!["calendar_events".to_string()];
        let calls = salvage_tool_calls("Sure, here's the function call:

<tools> { \"name\": \"calendar_events\", \"arguments\": { \"query\": \"Sam's party\" } } </tools>", &cal);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].arguments.contains("Sam's party"));
        assert_eq!(salvage_tool_calls("```json
{\"name\": \"calendar_events\", \"parameters\": {\"from\": \"today\"}}
```", &cal)[0].arguments, "{\"from\":\"today\"}");
        assert_eq!(salvage_tool_calls(" {\"name\": \"calendar_events\", \"arguments\": {}} ", &cal).len(), 1, "a reply that is only the call");
        // How qwen2.5-coder 14B wrote one: bare JSON after some words.
        let mail = vec!["email_read".to_string()];
        let (calls, rest) = salvage("Now, I'll read the email in full.
Let's read the email with the provided ID.
{\"name\": \"email_read\", \"arguments\": {\"id\": \"3b37\"}}", &mail);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, "{\"id\":\"3b37\"}");
        assert!(!rest.contains('{') && rest.starts_with("Now"), "{rest}");
        // A model that goes on to imagine more calls: only the first runs.
        let two = vec!["email_search".to_string(), "email_send".to_string()];
        let (calls, _) = salvage("<tool_call>{\"name\": \"email_search\", \"arguments\": {}}</tool_call>
Found it.
<tool_call>{\"name\": \"email_send\", \"arguments\": {\"to\": \"john@example.com\"}}</tool_call>", &two);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "email_search");
        assert!(salvage_tool_calls("```json
{\"name\": \"Sam\", \"age\": 3}
```", &cal).is_empty(), "ordinary JSON is left alone");
        let mut out = Finished { content: "Here:
```json
{\"a\": 1}
```
<tools>{\"name\": \"calendar_events\", \"arguments\": {}}</tools>".into(), thinking: String::new(), tool_calls: vec![], prompt_tokens: None, completion_tokens: None, tps: None, cancelled: false, raw: None };
        let tools = json!([{ "type": "function", "function": { "name": "calendar_events" } }]);
        rescue_tool_calls(&mut out, Some(&tools));
        assert_eq!(out.tool_calls.len(), 1);
        assert!(out.content.contains("\"a\": 1"), "other code blocks stay in the reply: {}", out.content);
    }

    #[test]
    fn a_reply_stuck_on_one_line_is_noticed() {
        let line = "*(I'm here to help - just say the date and time!)*\n";
        assert!(repeats_itself(&format!("Hello.\n{}", line.repeat(4))));
        assert!(!repeats_itself(&format!("Hello.\n{}", line.repeat(3))));
        assert!(!repeats_itself("1. a\n2. b\n3. c\n4. d\n5. e\n"));
        assert!(!repeats_itself("---\n---\n---\n---\n"), "short lines like rules are fine");
    }

    #[test]
    fn notices_a_model_asking_instead_of_looking() {
        // What qwen2.5-coder said.
        assert!(stalls("Summarize my most recent email from Google", "To summarize your most recent email from Google, I'll need to search your emails for the latest one from Google. Could you please confirm if you have any specific criteria or details you'd like me to look for in that email?", &[]));
        assert!(stalls("When is sam's party?", "To find out when Sam's party is, I'll need to search your calendar for any events related to Sam's party. Can you please provide me with the name of the event or any details that might help me locate it?", &[]));
        assert!(stalls("Summarize my most recent email from Google", "I don't have access to your email to read the most recent one. However, I can help you with other tasks or answer questions based on the information you provide. Let me know how I can assist you!", &[]));
        // Real answers, and questions that aren't about the user's things, are left alone.
        assert!(!stalls("When is sam's party?", "Sam's party is today at 6:00 PM at Sam's house.", &[]));
        assert!(!stalls("What's a closure in Rust?", "Could you mean closures as in Fn traits? A closure is...", &[]));
        assert!(!stalls("Write a note about my day", "", &[]));
        assert!(stalls("What did my mom ask me?", "I don\u{2019}t have access to your email or any messages you\u{2019}ve sent or received unless you\u{2019}ve shared them.", &[]));
        assert!(stalls("When is sam's party?", &format!("I don't have any information about Sam's party in my tools or memory. {}", "More options. ".repeat(100)), &[]), "a long ramble");
        assert!(stalls("What did my mom ask me?", "I'm sorry, but I don't have any information about what your mom asked you. I don't have access to any personal communications or messages outside of our current conversation.", &[]));
        assert!(!stalls("Write a note about my day", "Here's a note:\n\nA calm day: coffee, a long walk and some reading.\n\nWant me to save it to your notes?", &[]));
        assert!(stalls("Summarize my most recent email from Google", "I don't have direct access to your email inbox. You can check your email by using the `email_search` tool.", &["email_search".to_string()]));
        assert!(stalls("anything", "Use email_search with a query like from:google.", &["email_search".to_string()]), "naming a tool instead of calling it");
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
        let p = Profile { name: "Sam".into(), about: "I teach biology.".into(), preferences: "Be brief.".into(), ..Default::default() };
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

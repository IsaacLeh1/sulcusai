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
        "You are SulcusAI, a helpful assistant. Be clear, accurate and friendly. \
         Use Markdown when it helps; write math formulas in LaTeX, $...$ in a sentence and $$...$$ on their own lines. \
         If you don't know something, say so.\n\n\
         When a question needs facts you don't have, get them yourself with your tools (weather, web search, \
         notes, tasks, calendar, email, files) instead of asking the user. Ask only for what you can't look up, \
         and never guess figures. If they mention a name, title or term you don't recognize or aren't sure about \
         (a game, a character, a show, a product, slang), don't guess or make it up: look it up first.\n\nToday's date is {today}."
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

/// Where the model answering runs, so it never tells the user something
/// stays on their PC when it doesn't: on this PC (only what its online tools
/// send leaves it), or at the cloud provider the user picked.
pub fn runs_where(ep: &Endpoint) -> String {
    match &ep.cloud {
        Some(t) => format!(
            "You run on {p}'s servers, an online AI service the user chose for this chat: everything in this conversation is sent to {p} to answer it. \
             Never tell the user it stays on their PC.",
            p = t.provider
        ),
        None => "You run privately on the user's own computer: this conversation stays on their PC. \
                 Only what your online tools send (web searches, pages, email) leaves it."
            .into(),
    }
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
    // The instructions and the tool list come back every step, unchanged:
    // counted once per engine run.
    let key = (text.len() > 400).then(|| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        format!("{}:text:{:x}", ep.port, h.finish())
    });
    if let Some(n) = key.as_ref().and_then(|k| cache().lock().unwrap().get(k).copied()) {
        return Ok(n);
    }
    let n = tokenize(ep, text).await?;
    if let Some(k) = key {
        cache().lock().unwrap().insert(k, n);
    }
    Ok(n)
}

async fn tokenize(ep: &Endpoint, text: &str) -> Result<u32, String> {
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
    // The text is part of the key: a shortened copy of a message is counted again.
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        m.content.hash(&mut h);
        for c in m.tool_calls.iter().flatten() {
            c.arguments.hash(&mut h);
        }
        format!("{}:{}:{:x}", ep.port, m.id, h.finish())
    };
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

/// History with what's already in files left out: the contents of files it
/// wrote or edited in earlier steps, and long code it pasted into the chat
/// alongside them. A site's three files otherwise filled a small model's
/// memory twice over (pasted and written) by the second file. Only what's
/// sent changes; the newest message stays whole.
///
/// The contents are dropped, not replaced with a note: given a stand-in
/// ("[written to the file]"), Qwen2.5-Coder 14B wrote stand-ins into the
/// next files. Copying a call with no contents fails loudly instead.
pub fn without_written(history: &[Message]) -> Vec<Message> {
    static FENCE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)```[^\n]*\n(.*?)```").unwrap());
    let last = history.len().saturating_sub(1);
    history
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if i == last || m.role != "assistant" {
                return m.clone();
            }
            let mut m = m.clone();
            let mut wrote = false;
            if let Some(calls) = m.tool_calls.as_mut() {
                for c in calls.iter_mut().filter(|c| matches!(c.name.as_str(), "write_file" | "edit_file" | "create_document")) {
                    let Ok(mut args) = serde_json::from_str::<Value>(&c.arguments) else { continue };
                    for key in ["content", "new_text", "old_text", "markdown"] {
                        if args.get(key).and_then(Value::as_str).is_some_and(|s| s.len() > 300) {
                            if let Some(o) = args.as_object_mut() {
                                o.remove(key);
                            }
                            wrote = true;
                        }
                    }
                    c.arguments = args.to_string();
                }
            }
            // Code it pasted next to a file it wrote, or before writing one.
            let next_writes = history.get(i + 1..).into_iter().flatten().take(4).any(|n| n.tool_calls.iter().flatten().any(|c| c.name == "write_file"));
            // Left out without a note: given "[72 lines of code, in the files]" it
            // wrote that into its next reply instead of code.
            if wrote || next_writes {
                m.content = FENCE
                    .replace_all(&m.content, |c: &regex::Captures| if c[1].lines().count() > 12 { String::new() } else { c[0].to_string() })
                    .into_owned();
            }
            m
        })
        .collect()
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
            // Shrink the current turn's oldest tool outputs until it fits. The
            // results of the latest calls stay: without them the model runs
            // the same calls again (and the repeat guard refuses them).
            let mut group = group.to_vec();
            let mut total = total;
            let latest = group.iter().rposition(|m| m.role == "assistant" && m.tool_calls.as_ref().is_some_and(|c| !c.is_empty())).unwrap_or(group.len());
            let stub = count_tokens(ep, TRIMMED_TOOL_OUTPUT).await? + PER_MESSAGE_OVERHEAD;
            for (i, (m, size)) in group.iter_mut().zip(sizes.iter()).enumerate() {
                if total <= budget {
                    break;
                }
                if i < latest && m.role == "tool" && *size > stub {
                    m.content = TRIMMED_TOOL_OUTPUT.into();
                    total = total - size + stub;
                }
            }
            // Still too big: its own long earlier text in this turn ("Step 14:
            // write the files again…") is cut to its first lines. The newest
            // message stays whole.
            let last = group.len().saturating_sub(1);
            for i in 0..last {
                if total <= budget {
                    break;
                }
                let m = &mut group[i];
                if m.role == "assistant" && m.content.chars().count() > 400 {
                    let before = message_tokens(ep, m).await?;
                    let head: String = m.content.chars().take(300).collect();
                    m.content = format!("{head}…");
                    let after = message_tokens(ep, m).await?;
                    total = total.saturating_sub(before.saturating_sub(after));
                }
            }
            // Still too big: the latest results are cut to what's left, the
            // start and end of each kept (the biggest first).
            if total > budget {
                let mut order: Vec<usize> = (latest..group.len()).filter(|&i| group[i].role == "tool").collect();
                order.sort_by_key(|&i| std::cmp::Reverse(group[i].content.len()));
                for i in order {
                    if total <= budget {
                        break;
                    }
                    let before = message_tokens(ep, &group[i]).await?;
                    let over = total - budget;
                    let keep_tokens = before.saturating_sub(over).max(200);
                    let chars = group[i].content.chars().count();
                    // About 3 characters a token.
                    let keep = (keep_tokens as usize * 3).min(chars);
                    if keep >= chars {
                        continue;
                    }
                    group[i].content = cut_middle(&group[i].content, keep);
                    let after = message_tokens(ep, &group[i]).await?;
                    total = total.saturating_sub(before.saturating_sub(after));
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

/// The start and end of a long text, `keep` characters in all, with a note
/// of what was left out.
fn cut_middle(text: &str, keep: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let head = keep * 2 / 3;
    let tail = keep - head;
    let start: String = chars[..head.min(chars.len())].iter().collect();
    let end: String = chars[chars.len().saturating_sub(tail)..].iter().collect();
    format!("{start}\n…[{} characters left out to make room]…\n{end}", chars.len().saturating_sub(keep))
}

/// A tool output that was removed to make room (the model may run it again).
pub fn is_trimmed(m: &Message) -> bool {
    m.role == "tool" && m.content == TRIMMED_TOOL_OUTPUT
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
                let mut turn = json!({ "role": "assistant", "content": m.content, "tool_calls": calls });
                // Its reasoning before the call goes back with it: gpt-oss's
                // template shows it again for the steps of the current task,
                // so the model keeps its plan between tool calls (the engine
                // on this PC only; `stream` takes it out for online models).
                if let Some(t) = m.thinking.as_deref().filter(|t| !t.trim().is_empty()) {
                    turn["reasoning_content"] = json!(t);
                }
                out.push(turn);
            }
            // In a chat the user shares: someone it's shared with wrote this,
            // or their model answered them (on their PC, not this model).
            _ if m.meta.as_ref().is_some_and(|v| v.get("from_person").is_some()) => {
                let meta = m.meta.as_ref();
                let name = meta.and_then(|v| v.get("from_name")).and_then(Value::as_str).unwrap_or("Someone");
                let who = if m.role == "assistant" {
                    let model = meta.and_then(|v| v.get("model")).and_then(Value::as_str).unwrap_or("their model");
                    format!("[{name}'s assistant ({model}, on {name}'s PC) answered {name}:]")
                } else {
                    format!("[{name}, whom the user shares this chat with, wrote:]")
                };
                out.push(json!({ "role": "user", "content": format!("{}\n{who}\n{}", sent_note(m.created_at), m.content) }));
            }
            // When each message was sent, so "this afternoon" or "tonight"
            // make sense. Fixed per message, so the engine's cache holds.
            // Asked by the assistant of another chat, not typed by the user.
            "user" if m.meta.as_ref().is_some_and(|v| v.get("from_chat").is_some()) => {
                let from = m.meta.as_ref().and_then(|v| v.get("from_title")).and_then(Value::as_str).unwrap_or("another chat");
                out.push(json!({ "role": "user", "content": format!(
                    "{}\n[From the assistant in the user's chat \"{from}\", working for the user. Do what it asks here, then answer it: your reply goes back to it.]\n{}",
                    sent_note(m.created_at), m.content
                ) }));
            }
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
/// `see`, the model is told it can't see them, so it can say so; with
/// `can_edit`, that it can still change them with edit_image (a model
/// that can't see a photo told to say so refused "add a top hat to me").
/// `described`: what the picture reader saw in pictures (by id), for a model
/// that can't see them: it gets those words instead.
pub fn attach_images(out: &mut [Value], history: &[Message], see: Option<&dyn Fn(&str) -> Option<String>>, can_edit: bool, described: &[(String, String)]) {
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
        let words: Vec<String> = ids
            .iter()
            .enumerate()
            .filter_map(|(k, id)| described.iter().find(|(d, _)| d == id).map(|(_, w)| format!("Picture {} (id {id}): {w}", k + 1)))
            .collect();
        if see.is_none() && !words.is_empty() {
            let edit = if can_edit { " To change one, call edit_image with its id and the change." } else { "" };
            slot["content"] = json!(format!(
                "{text}\n[The user attached {n} picture(s). You can't see pictures yourself, but the picture reader looked at them for you; \
                 answer from what it saw as if you had seen them.{edit}\n{}]",
                words.join("\n")
            ));
            continue;
        }
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
            None if can_edit => json!(format!(
                "{text}\n[The user attached {n} picture(s) (id: {}). You can't see them, but you can still change them: for an edit \
                 (add or remove something, change the background, the look or the time of day) call edit_image with their \
                 instruction and that id. Only if they ask what's in it, say you can't see pictures and suggest a model that \
                 can, such as Qwen3 VL or Gemma 3.]",
                ids.join(", ")
            )),
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
        let messages = messages
            .into_iter()
            .map(|mut m| {
                if let Some(o) = m.as_object_mut() {
                    o.remove("reasoning_content");
                }
                m
            })
            .collect();
        return crate::cloud::stream(t, &ep.extra, messages, tools, cancel, on_delta).await;
    }
    let (first, cut_off) = stream_local(ep, messages.clone(), tools, cancel, &mut on_delta).await?;
    // A thinking model that spent its whole allowance thinking and never
    // answered: once more, without thinking.
    if cut_off && first.content.trim().is_empty() && first.tool_calls.is_empty() && !first.thinking.trim().is_empty() && !first.cancelled {
        let mut quick = ep.clone();
        quick.extra["chat_template_kwargs"] = json!({ "enable_thinking": false });
        quick.extra["reasoning_effort"] = json!("low");
        let (mut again, _) = stream_local(&quick, messages, tools, cancel, &mut on_delta).await?;
        again.thinking = format!("{}{}", first.thinking, again.thinking);
        if again.content.trim().is_empty() && again.tool_calls.is_empty() && !again.cancelled {
            return Err("The model spent its whole reply thinking and never answered. Try again, or turn its thinking off in Advanced settings.".into());
        }
        return Ok(again);
    }
    Ok(first)
}

/// One reply from the engine on this PC, and whether it stopped at its
/// length limit.
async fn stream_local<F: FnMut(Delta)>(ep: &Endpoint, messages: Vec<Value>, tools: Option<&Value>, cancel: &AtomicBool, on_delta: &mut F) -> Result<(Finished, bool), String> {
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
    // Bytes until a whole line is in: a character split between two chunks
    // would otherwise come out garbled.
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    let offered = offered_names(tools);
    let mut cut_off = false;
    'read: while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        let chunk = chunk.map_err(|e| format!("The reply was interrupted: {e}"))?;
        buf.extend_from_slice(&chunk);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let bytes: Vec<u8> = buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&bytes);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break 'read;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if v.pointer("/choices/0/finish_reason").and_then(Value::as_str) == Some("length") {
                cut_off = true;
            }
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
    // gpt-oss calling its built-in tools (browser.search, python): as ours.
    let offered = offered_names(tools);
    for c in out.tool_calls.iter_mut().filter(|c| !offered.contains(&c.name)) {
        let args: Value = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
        if let Some((name, args)) = harmony_alias(&c.name, &args).filter(|(n, _)| offered.contains(n)) {
            c.name = name;
            c.arguments = args.to_string();
        }
    }
    for (i, c) in out.tool_calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{}_{i}", uuid::Uuid::new_v4().simple());
        }
    }
    out.tool_calls.retain(|c| !c.name.is_empty());
    Ok((out, cut_off))
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

/// gpt-oss's built-in tools, as SulcusAI's: it was trained to search and
/// open pages with `browser` and to run code with `python`, and calls them
/// even when they aren't offered.
pub(crate) fn harmony_alias(name: &str, args: &Value) -> Option<(String, Value)> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    match name {
        "browser.search" | "browser_search" => Some(("web_search".into(), json!({ "query": s("query")? }))),
        "browser.open" | "browser_open_url" => Some(("fetch_page".into(), json!({ "url": s("url").or_else(|| s("id").filter(|i| i.starts_with("http")))? }))),
        "python" => Some(("run_code".into(), json!({ "code": s("code").or_else(|| s("input"))? }))),
        "container.exec" => {
            let cmd = args.get("cmd").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).or_else(|| s("cmd"))?;
            Some(("run_command".into(), json!({ "command": cmd })))
        }
        _ => None,
    }
}

/// The JSON object starting at `text`'s first `{`, with braces inside
/// strings ignored; and where it ends.
fn json_object_at(text: &str) -> Option<(usize, usize)> {
    let start = text.find('{')?;
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (i, c) in text[start..].char_indices() {
        match c {
            _ if esc => esc = false,
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, start + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

static HARMONY_CALL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"to=(?:functions\.)?([A-Za-z_][\w.\-]*)").unwrap());
static HARMONY_TOKEN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"<\|(?:start|end|message|channel|constrain|call|return)\|>").unwrap());

/// Tool calls written in gpt-oss's Harmony format as plain text
/// (`<|channel|>commentary to=functions.find_files <|constrain|>json<|message|>{…}`,
/// or the same with the special tokens left out), which the engine leaves
/// in the reply when it doesn't parse them: run them, and keep the rest of
/// the text without Harmony's markers.
pub(crate) fn harmony_calls(text: &str, offered: &[String]) -> (Vec<ToolCall>, String) {
    if !text.contains("to=") {
        return (Vec::new(), text.to_string());
    }
    let mut calls = Vec::new();
    let mut rest = String::new();
    let mut at = 0;
    while let Some(m) = HARMONY_CALL.captures(&text[at..]) {
        let whole = m.get(0).unwrap();
        let name = m[1].trim_end_matches('.').to_string();
        let after = at + whole.end();
        let Some((s, e)) = json_object_at(&text[after..]) else { break };
        // Only the channel and format words may sit between the name and its arguments.
        let between = HARMONY_TOKEN.replace_all(&text[after..after + s], " ");
        if between.split_whitespace().any(|w| !matches!(w, "commentary" | "json" | "<|constrain|>json" | "code")) {
            rest.push_str(&text[at..after]);
            at = after;
            continue;
        }
        let raw = &text[after + s..after + e];
        let args: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        let (name, args) = if offered.iter().any(|o| *o == name) {
            (name, args)
        } else if let Some((n, a)) = harmony_alias(&name, &args).filter(|(n, _)| offered.iter().any(|o| o == n)) {
            (n, a)
        } else {
            rest.push_str(&text[at..after + e]);
            at = after + e;
            continue;
        };
        // The channel word just before the call goes with it.
        let before = text[at..at + whole.start()].trim_end();
        rest.push_str(before.strip_suffix("commentary").unwrap_or(before));
        rest.push(' ');
        calls.push(ToolCall { id: String::new(), name, arguments: if args.is_null() { raw.to_string() } else { args.to_string() } });
        at = after + e;
    }
    rest.push_str(&text[at..]);
    let rest = HARMONY_TOKEN.replace_all(&rest, " ");
    let rest = rest.replace("assistantcommentary", " ").replace("assistantanalysis", " ").replace("assistantfinal", " ");
    (calls, rest.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string())
}

/// Small models sometimes write a tool call into their thinking or their
/// text instead of the tool-call channel; run it rather than lose it.
pub(crate) fn rescue_tool_calls(out: &mut Finished, tools: Option<&Value>) {
    if out.tool_calls.is_empty() && !out.cancelled {
        let offered = offered_names(tools);
        // gpt-oss's own format, left as text by the engine.
        if !offered.is_empty() {
            let (calls, rest) = harmony_calls(&out.content, &offered);
            if !calls.is_empty() {
                out.content = rest;
                out.tool_calls = calls;
                return;
            }
            let (calls, _) = harmony_calls(&out.thinking, &offered);
            if !calls.is_empty() && out.content.trim().is_empty() {
                out.tool_calls = calls;
                return;
            }
        }
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
    regex::Regex::new(r"(?i)\b(e-?mails?|mail|inbox|messages?|calendar|events?|schedule|meetings?|appointments?|party|birthday|today|tonight|tomorrow|this week|next week|notes?|tasks?|to-?dos?|reminders?|weather|forecast|near me|nearby|nearest|closest|around here)\b").unwrap()
});
// "I don't have access to your messages": wrong whenever tools were offered,
// whatever the question.
static NO_ACCESS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)(don'?t|do not|can'?t|cannot) (have )?(direct |any |real-time |personal )?access|(unable|not able) to (access|read|check|see)|i (can'?t|cannot) (read|check|see) your|i don'?t have (any )?(information|details) about|without you providing|i don'?t have any (email|calendar|accounts?)").unwrap()
});
static STALLING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)i'?ll need to|i need to|i will need|i would need|please (provide|confirm|specify)|(could|can) you (please )?(provide|confirm|specify|tell me|share|give me)|(any|more|specific) (details|criteria|information)|function call|<tool|(i'?ll|i will|let me) (search|look|check)|(don'?t|do not|can'?t|cannot) (have )?access|(unable|not able) to (access|read|check|see)|i (can'?t|cannot) (read|check|see)").unwrap()
});

// "I'm not familiar with that", "as of my knowledge cutoff": it doesn't know
// what was asked about, which a web search would tell it.
static NOT_KNOWN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(i'?m|i am) (not |un)familiar\b|\bnot (familiar|aware) with\b|\b(i'?m|i am) not (sure|certain) (what|who|which) .{0,40}\b(is|are|was|were|refers? to|means?)\b|\bi (don'?t|do not) (know|recognize|recognise) (what|who) .{0,40}\b(is|are|was|were|refers? to|means?)\b|\bi (don'?t|do not) (know|recognize|recognise) (of |anything about )|\bi (don'?t|do not) have (any )?(information|details|knowledge) (on|about|of) (that|this|it|the)\b|\b(as of|beyond|after|past) my (last )?(knowledge|training) (cutoff|cut-off|date|update)|\bmy (knowledge|training data) (only goes|doesn'?t|does not|ends|is limited)|\bi couldn'?t find (any )?information|\b(could|can) you (clarify|explain|tell me) (what|who) .{0,40}\b(is|are|means?)\b").unwrap()
});

/// The reply says it doesn't know what (or who) the user means.
pub fn doesnt_know(reply: &str) -> bool {
    let reply = reply.trim().replace(['\u{2019}', '\u{2018}'], "'");
    let opening: String = reply.chars().take(600).collect();
    NOT_KNOWN.is_match(&opening)
}

/// For a model that doesn't know what it was asked about: one call that looks
/// it up on the web (or asks to, when the web is off for this chat).
pub fn forced_web(tools: &Value) -> Option<ForcedLookup> {
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let all = tools.as_array()?;
    let pick = if all.iter().any(|t| name_of(t) == "web_search") { "web_search" } else if all.iter().any(|t| name_of(t) == "request_web") { "request_web" } else { return None };
    let def = all.iter().find(|t| name_of(t) == pick)?;
    let schema = json!({ "type": "object", "properties": { "name": { "const": pick }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
    let what = if pick == "web_search" {
        "search the web for it now with web_search (a short query naming it)"
    } else {
        "ask to look it up with request_web (the reason names what you'd look up)"
    };
    Some(ForcedLookup {
        names: vec![pick.into()],
        nudge: format!("(Don't guess and don't ask me to explain it: {what}. Reply with only one tool call as JSON, {{\"name\": \"{pick}\", \"arguments\": {{...}}}}.)"),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

// A sum to work out ("what's 37*48", "15% of 2,345", "1250 divided by 7",
// "how many days until Christmas"), which small models get wrong in their heads.
// Dates (10/17, 2026-10-17) and sizes (3x3) aren't sums.
static MATH_ASK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\d\s*[*×÷^]\s*\(?\d|\d\s+[-+/]\s+\(?\d|\d\s*\+\s*\d|\b\d[\d,.]*\s*(plus|minus|times|divided by|multiplied by|to the power of)\s+\d|\b\d+(\.\d+)?\s*(%|percent)\s+of\s+\$?\d|\b(square root|cube root|sqrt) of\s+\d|\bhow many (days|weeks|months|hours|minutes|seconds) (until|till|between|since|from|are (there )?in|left)\b|\b(compound interest|monthly payment|mortgage payment|loan payment)\b|\btip on \$?\d|\bsplit \$?\d[\d,.]* (\w+ )?(ways|between)\b|\bsolve\b[^.?!\n]*=|\b(integral|integrate|derivative|differentiate)\b|\b[a-z]\s*\^\s*\d",
    )
    .unwrap()
});

/// The message asks for something to be worked out (and isn't a request to build or write code).
pub fn asks_math(text: &str) -> bool {
    let t: String = text.chars().take(600).collect();
    MATH_ASK.is_match(&t) && !t.contains("```") && !is_build_ask(&[t.as_str()])
}

/// For a model that answered a sum from its head: one call that works it out.
pub fn forced_calc(tools: &Value) -> Option<ForcedLookup> {
    let def = tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some("calculate"))?;
    let schema = json!({ "type": "object", "properties": { "name": { "const": "calculate" }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
    Some(ForcedLookup {
        names: vec!["calculate".into()],
        nudge: "(Don't work it out in your head: call calculate with the math from my message as one expression, e.g. \"37*48\", \"15% of 2345\", \
                an equation as it is (\"x^2 - 5x + 6 = 0\"), \"integral of x^2 from 0 to 3\", \"derivative of x^3 at 2\" or \
                \"daysBetween('2026-10-17', '2026-12-25')\". Reply with only one tool call as JSON, {\"name\": \"calculate\", \"arguments\": {\"expression\": \"...\"}}.)"
            .into(),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

// Asking for a picture to be made.
static PICTURE_ASK:std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(make|create|draw|generate|paint|render|design|show me|give me|i want|can you (make|draw|create))\b[\s\S]{0,60}\b(picture|image|photo|drawing|painting|illustration|artwork|art|wallpaper|logo|portrait|sketch|pixel art|poster)\b|\b(picture|image|photo|drawing|painting) of\b|\b(draw|paint|sketch|illustrate)\s+(me\s+)?(a|an|the|some|my|our)\b").unwrap()
});
// Saying it can't make pictures (often right after making one).
static NO_PICTURES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(can'?t|cannot|can not|unable to|not able to|don'?t have the ability to)\b[^.!?\n]{0,30}\b(generate|create|make|draw|produce|display|show|edit|modify|change|alter)\b[^.!?\n]{0,20}\b(images?|pictures?|photos?|drawings?|art)").unwrap()
});

/// More than one picture (or clip, or song) asked for in one message.
pub fn wants_several(text: &str) -> bool {
    static SEVERAL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(pictures|images|photos|drawings|versions|variations|options|several|a few|some|both|each|every|all of|series|set of|[2-9]|two|three|four|five)\b").unwrap()
    });
    SEVERAL.is_match(text)
}

/// The person asked for a picture to be made.
pub fn asks_for_picture(text: &str) -> bool {
    PICTURE_ASK.is_match(text)
}

/// The reply says it can't make pictures.
pub fn denies_pictures(reply: &str) -> bool {
    NO_PICTURES.is_match(&reply.replace(['\u{2019}', '\u{2018}'], "'"))
}

// A change to an attached picture ("add a hat", "make me a cartoon", "remove the car").
static PICTURE_CHANGE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(add|put|give)\s+(a|an|some|the|me|him|her|them|us|it)\b|\b(remove|erase|replace)\s+(the|my|his|her|their|that|this)\b|\b(make|turn)\s+(it|this|me|him|her|them|us|the (photo|picture|image|background|sky))\b|\bchange\s+(the|my|his|her|their)\s+\w+|\b(restyle|edit (this|the|my) (photo|picture|image)|in the style of|as an? (cartoon|painting|drawing|anime))\b").unwrap()
});

/// The person asked for a change to a picture (with one attached).
pub fn asks_picture_change(text: &str) -> bool {
    PICTURE_CHANGE.is_match(text)
}

/// For a model that answered a picture request without making one: one call
/// that makes it, or, for a change to an attached picture, that changes it.
pub fn forced_picture(tools: &Value, change: bool) -> Option<ForcedLookup> {
    let find = |name: &str| tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some(name)).cloned();
    if change {
        if let Some(def) = find("edit_image") {
            let schema = json!({ "type": "object", "properties": { "name": { "const": "edit_image" }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
            return Some(ForcedLookup {
                names: vec!["edit_image".into()],
                nudge: "(You can change pictures on this PC: change the picture I attached now with edit_image, with the change as the instruction. Reply with only one tool call as JSON, {\"name\": \"edit_image\", \"arguments\": {...}}.)".into(),
                response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
            });
        }
    }
    let def = find("create_image")?;
    let schema = json!({ "type": "object", "properties": { "name": { "const": "create_image" }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
    Some(ForcedLookup {
        names: vec!["create_image".into()],
        nudge: "(You can make pictures on this PC: make it now with create_image, with a vivid description of what I asked for (spell out how anything named looks). Reply with only one tool call as JSON, {\"name\": \"create_image\", \"arguments\": {...}}.)".into(),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// A named thing in a picture request (a character, a game, a brand, a
/// person, a place) and what the model says it looks like.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Named {
    pub name: String,
    #[serde(default)]
    pub known: bool,
    #[serde(default)]
    pub looks: String,
}

impl Named {
    /// Sure of its look, with enough words to draw it.
    pub fn described(&self) -> bool {
        self.known && self.looks.split_whitespace().count() >= 4
    }
}

/// Asks the model which things in a picture request have a particular look
/// that picture models won't know by name, and what each looks like.
pub fn names_check(asked: &str, prompt: &str) -> (String, Value) {
    let ask = format!(
        "A picture is about to be made.\nWhat the user asked: \u{ab}{asked}\u{bb}\nThe description for the picture model: \u{ab}{prompt}\u{bb}\n\n\
         Picture models don't know names: they only draw what the words describe. List each specific named thing above that \
         has its own particular look: a character, a game, film, show or book, a brand, product or logo, a real person, a famous \
         place, building or artwork. Leave out ordinary things (a dog, a castle, a sword, a sunset). For each one: known is true \
         only if you are sure exactly what it looks like, and looks is that look in one line (colors, clothes, hair, shapes, \
         distinctive marks), or empty if you aren't sure. An empty list is fine. Reply with only the JSON."
    );
    let schema = json!({ "type": "object", "properties": { "names": { "type": "array", "maxItems": 6, "items": {
        "type": "object",
        "properties": { "name": { "type": "string" }, "known": { "type": "boolean" }, "looks": { "type": "string" } },
        "required": ["name", "known", "looks"] } } }, "required": ["names"] });
    (ask, json!({ "type": "json_schema", "json_schema": { "name": "names", "schema": schema } }))
}

/// The named things from a names check's reply (empty if it isn't JSON).
pub fn names_of(reply: &str) -> Vec<Named> {
    #[derive(serde::Deserialize)]
    struct Reply {
        #[serde(default)]
        names: Vec<Named>,
    }
    let all: Vec<Named> = serde_json::from_str::<Reply>(reply.trim()).map(|r| r.names).unwrap_or_default();
    let mut names: Vec<Named> = Vec::new();
    for n in all {
        if !n.name.trim().is_empty() && !names.iter().any(|k| k.name.trim().eq_ignore_ascii_case(n.name.trim())) {
            names.push(n);
        }
    }
    names.truncate(6);
    names
}

/// Only real names: capitalized as written in what the user asked or in the
/// description, and not an everyday word. (Asked about "put him at a party",
/// a small model listed "him", and the look it found on the web was HIM the
/// cartoon villain's: lobster claws and red skin on the user's grandfather.)
pub fn real_names(names: Vec<Named>, asked: &str, prompt: &str) -> Vec<Named> {
    const COMMON: &[&str] = &[
        "him", "her", "he", "she", "it", "they", "them", "his", "hers", "their", "me", "my", "you", "your", "we", "us", "our", "i",
        "man", "woman", "boy", "girl", "person", "people", "guy", "kid", "child", "baby", "dog", "cat", "picture", "photo", "image",
        "the man", "the woman", "old man", "old woman", "the picture", "this", "that", "someone", "everyone",
    ];
    names
        .into_iter()
        .filter(|n| {
            let name = n.name.trim();
            let lower = name.to_lowercase();
            name.chars().count() >= 2
                && !COMMON.contains(&lower.as_str())
                && name.chars().any(char::is_uppercase)
                && (asked.contains(name) || prompt.contains(name))
        })
        .collect()
}

/// Asks the model to describe a thing's look from web search results.
pub fn looks_from(name: &str, found: &str) -> (String, Value) {
    let ask = format!(
        "{found}\n\nFrom the search results above, describe how {name} looks, in one line for a picture model: colors, clothes, \
         hair, shapes and distinctive marks. Only what the results say or clearly show; empty if they say nothing about its look. \
         Reply with only the JSON."
    );
    let schema = json!({ "type": "object", "properties": { "looks": { "type": "string" } }, "required": ["looks"] });
    (ask, json!({ "type": "json_schema", "json_schema": { "name": "looks", "schema": schema } }))
}

/// The look from a `looks_from` reply.
pub fn looks_of(reply: &str) -> String {
    serde_json::from_str::<Value>(reply.trim()).ok().and_then(|v| v["looks"].as_str().map(|s| s.trim().to_string())).unwrap_or_default()
}

/// The picture description with each named thing's look spelled out after
/// it, since picture models draw words, not names.
pub fn with_looks(prompt: &str, names: &[Named]) -> String {
    let lower = prompt.to_lowercase();
    let added: Vec<String> = names
        .iter()
        .filter(|n| n.described())
        // Already described in the prompt itself.
        .filter(|n| !lower.contains(&n.looks.to_lowercase()))
        .map(|n| format!("{} looks like this: {}", n.name.trim(), n.looks.trim().trim_end_matches('.')))
        .collect();
    if added.is_empty() {
        return prompt.to_string();
    }
    let p = prompt.trim().trim_end_matches('.');
    format!("{p}. {}.", added.join(". "))
}

/// The reply without its code (fenced blocks and `inline code`).
fn without_code(reply: &str) -> String {
    let mut out = String::with_capacity(reply.len());
    for (i, part) in reply.split("```").enumerate() {
        if i % 2 == 0 {
            for (j, bit) in part.split('`').enumerate() {
                if j % 2 == 0 {
                    out.push_str(bit);
                }
            }
        }
    }
    out
}

/// A small model asked the user for details or permission about their own
/// mail, calendar, notes, tasks or weather instead of using a tool to look.
pub fn stalls(user: &str, reply: &str, offered: &[String]) -> bool {
    let reply = reply.trim().replace(['\u{2019}', '\u{2018}'], "'");
    if reply.is_empty() {
        return false;
    }
    // Telling the user to "use email_search" instead of using it. Not in
    // code it wrote (a function of the same name) or as a call there ("name(").
    let prose = without_code(&reply);
    if offered.iter().any(|n| n.contains('_') && prose.match_indices(n.as_str()).any(|(i, _)| !prose[i + n.len()..].starts_with('('))) {
        return true;
    }
    // Saying it has no access is wrong however long the reply goes on.
    let opening: String = reply.chars().take(600).collect();
    // Ending on what it's about to look up, without looking ("Here are the
    // searches I'll run: … Let's execute these searches", the live test's 14B).
    let n = reply.chars().count();
    let ending: String = reply.chars().skip(n.saturating_sub(250)).collect();
    NO_ACCESS.is_match(&opening) || (n < 800 && ABOUT_THEIR_THINGS.is_match(user) && STALLING.is_match(&reply)) || (n < 1500 && ANNOUNCES.is_match(&ending))
}

static ANNOUNCES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(i'?ll|i will|let me|let'?s|let us|i'?m going to|i am going to)\s+(now\s+)?(perform|run|do|execute|conduct|start|carry out|search|look|check|research|find|gather|fetch|browse)\b[^\n]{0,80}\b(search(es)?|research|look ?ups?|the web|online|sources|information|results|data)\b[^\n]{0,40}[.:!]?\s*$").unwrap()
});

// Asking for something to be made: a site, an app, a script, files.
static BUILD_ASK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(make|create|build|write|generate|code|program|develop|scaffold|set up|save|put)\b[\s\S]{0,80}\b(files?|folders?|website|web ?site|web ?page|page|site|app|application|script|program|game|project|component|extension|bot|server|api|tool|calculator|html|css|javascript|python|react|template|readme|\.[a-z]{1,4}\b)").unwrap()
});
// Steps for the person to follow instead of doing it.
static HANDS_OVER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(create (a|an|the) (new )?(file|folder|directory)|save (this|it|the (code|file)) as|copy (and paste |the following|this)|paste (this|it|the following)|open (your|a) (terminal|editor|text editor|command prompt)|step 1\b|here'?s (the|a|some) (code|html|script))").unwrap()
});

// "Do it for me" after asking for something to be made.
static DO_IT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(do (it|this|that|it all|all (of )?(it|this|that))|try again|go ahead|build it|make it|run it|on my (pc|computer|laptop))\b").unwrap()
});
// Saying it can't make or run things, which it can here.
static CANT_DO: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\bas an ai\b|\bi (can't|cannot|can not|am unable to|'m unable to|am not able to|don't have the ability to|do not have the ability to) (directly )?(execute|run|create|make|build|access|write|set up|save)\b|\b(can't|cannot) (create|write|save) files\b").unwrap()
});
// Asking for a document (Word, PDF, Excel, PowerPoint, Markdown), which create_document makes.
static DOCUMENT_ASK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(docx?|word (doc|document|file)|pdf|excel|spreadsheet|xlsx|powerpoint|slides?|deck|pptx|report|resume|cv|letter|memo|markdown)\b|\.md\b").unwrap()
});

/// The person asked for something to be made: in their newest message, or
/// earlier with the newest saying "do it" ("do it for me", "try again").
/// `recent_user` is their last few messages, newest first.
pub fn is_build_ask(recent_user: &[&str]) -> bool {
    let Some((newest, earlier)) = recent_user.split_first() else { return false };
    BUILD_ASK.is_match(newest) || (DO_IT.is_match(newest) && earlier.iter().any(|m| BUILD_ASK.is_match(m)))
}

/// Asking for a document rather than a site, an app or code.
pub fn wants_a_document(text: &str) -> bool {
    DOCUMENT_ASK.is_match(text)
}

/// A short reply that only asks for details ("What kind of features do you
/// need?") instead of making a first version.
fn only_asks(reply: &str) -> bool {
    static ASKS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(what (kind|sort|type)s? of|which (features|kind|type|style|framework|language|pages)|what (features|would you like|do you (want|need|have in mind))|do you (want|need|have|prefer)|would you like|(could|can) you (tell|share|provide|describe|give)|let me know)\b").unwrap()
    });
    let r = reply.trim();
    r.chars().count() < 600 && r.contains('?') && (ASKS.is_match(r) || STALLING.is_match(r))
}

/// A request to make something answered with code, steps, "I can't" or
/// questions in the chat, though tools to make it on the PC were offered.
/// Said to a model that explained steps instead of carrying them out.
pub const CARRY_ON: &str = "Don't explain the steps or ask whether to go ahead: you already have permission. Carry them out now with \
     your tools, one after another, then say briefly what you did and what happened.";

/// The person asked for something to be done (not explained): apply, run,
/// install, follow, set up, fix, make…
pub fn asks_to_act(user: &str) -> bool {
    static ACT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(apply|applies|do (it|this|that|them|these|the steps)|run|execute|install|set (it |this |them )?up|setup|follow|carry (it |them )?out|implement|fix|change|update|configure|move|copy|rename|delete|download|open|launch|start|make|create|build|find (and|then)|go ahead)\b").unwrap()
    });
    ACT.is_match(user)
}

/// A reply that tells the user how to do it (numbered steps, "you'll need
/// to", "run the following") or asks whether to go ahead, instead of doing it.
pub fn explains_instead(reply: &str) -> bool {
    static NUMBERED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?m)^\s*(\*\*)?(1[.)]|step 1\b)").unwrap());
    static TELLS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(you('ll| will)? (need|have) to|you (can|should|could) (run|open|create|install|copy|paste|type|edit|add|change|go|click|navigate|download|use)|run the following|follow these steps|here('s| is| are) (how|the steps)|steps to|open (a |your )?(terminal|command prompt|powershell|file explorer)|(should|shall|may|can) i (go ahead|proceed|start|run|do|apply|continue)|would you like me to|do you want me to|want me to (go ahead|proceed|run|do|apply)|let me know (if|when) you('d| would)? (like|want))\b").unwrap()
    });
    let prose = without_code(reply);
    let numbered_steps = NUMBERED.is_match(&prose) && prose.lines().filter(|l| l.trim_start().starts_with("2.") || l.trim_start().starts_with("2)")).count() > 0;
    numbered_steps || TELLS.is_match(&prose) || HANDS_OVER.is_match(&prose)
}

pub fn builds_in_chat(recent_user: &[&str], reply: &str, offered: &[String]) -> bool {
    let can_make = offered.iter().any(|n| n == "write_file" || n == "request_folder");
    let code_lines = reply.split("```").skip(1).step_by(2).map(|block| block.lines().count()).sum::<usize>();
    can_make && is_build_ask(recent_user) && (code_lines >= 4 || HANDS_OVER.is_match(reply) || CANT_DO.is_match(reply) || only_asks(reply))
}

/// Files a page it wrote uses but that aren't there yet: (the page, the
/// missing file's full path). Only stylesheets and scripts, which it can write.
pub fn missing_pieces(written: &[std::path::PathBuf]) -> Vec<(String, std::path::PathBuf)> {
    static REF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r#"(?i)(?:href|src)\s*=\s*["']([^"'#?]+\.(?:css|js|mjs))["']"#).unwrap());
    let mut out: Vec<(String, std::path::PathBuf)> = Vec::new();
    for page in written.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))) {
        let Ok(text) = std::fs::read_to_string(page) else { continue };
        let Some(dir) = page.parent() else { continue };
        for c in REF.captures_iter(&text) {
            let r = &c[1];
            if r.contains("://") || r.starts_with("//") || r.starts_with('/') || r.contains("..") {
                continue;
            }
            let full = dir.join(r);
            if !full.exists() && !out.iter().any(|(_, p)| *p == full) {
                out.push((page.file_name().unwrap_or_default().to_string_lossy().into_owned(), full));
            }
        }
    }
    out
}

/// One call that writes a file a page needs (its path fixed in the schema).
pub fn forced_write(tools: &Value, page: &str, missing: &std::path::Path) -> Option<ForcedLookup> {
    let def = tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some("write_file"))?;
    let path = missing.display().to_string();
    let mut params = def["function"]["parameters"].clone();
    params["properties"]["path"] = json!({ "type": "string", "const": path });
    let schema = json!({ "type": "object", "properties": { "name": { "const": "write_file" }, "arguments": params }, "required": ["name", "arguments"] });
    let name = missing.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Some(ForcedLookup {
        names: vec!["write_file".into()],
        nudge: format!(
            "({page} uses {name}, which doesn't exist yet, so the page won't work. Write it now with write_file (path {path:?}): its full, working contents to go with {page}. Reply with only one tool call as JSON, {{\"name\": \"write_file\", \"arguments\": {{...}}}}.)"
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// The reply has a long block of code in it (more than 12 lines).
pub fn pasted_code(reply: &str) -> bool {
    static FENCE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)```[^\n]*\n(.*?)(```|$)").unwrap());
    FENCE.captures_iter(reply).any(|c| c[1].lines().count() > 12)
}

/// The file a reply is about, among those written this turn: the one it
/// names ("update `app.js`"), else the newest script, else the newest file.
pub fn named_file(reply: &str, written: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    let low = reply.to_lowercase();
    written
        .iter()
        .rev()
        .find(|p| p.file_name().is_some_and(|n| low.contains(&n.to_string_lossy().to_lowercase())))
        .or_else(|| written.iter().rev().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("js"))))
        .or_else(|| written.last())
        .cloned()
}

/// For a model that wrote a fix into its reply and previewed the unchanged
/// site again: one call that saves the file, whole.
pub fn forced_save(tools: &Value, file: &std::path::Path) -> Option<ForcedLookup> {
    let mut f = forced_write(tools, "", file)?;
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    f.nudge = format!(
        "(The {name} you wrote in your reply was never saved, so the site didn't change. Save it now with write_file (path {:?}): the whole file, with every fix and every feature. Reply with only one tool call as JSON, {{\"name\": \"write_file\", \"arguments\": {{...}}}}.)",
        file.display().to_string()
    );
    Some(f)
}

/// For a model that wrote a site and stopped: one call that previews it (the
/// folder fixed in the schema), which opens it beside the chat and checks it.
pub fn forced_preview(tools: &Value, folder: &str) -> Option<ForcedLookup> {
    let def = tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some("preview_site"))?;
    let mut params = def["function"]["parameters"].clone();
    params["properties"] = json!({ "path": { "type": "string", "const": folder } });
    params["required"] = json!(["path"]);
    let schema = json!({ "type": "object", "properties": { "name": { "const": "preview_site" }, "arguments": params }, "required": ["name", "arguments"] });
    Some(ForcedLookup {
        names: vec!["preview_site".into()],
        nudge: format!(
            "(Before you finish, open the site and check that it works: call preview_site with path {folder:?}. Reply with only one tool call as JSON, {{\"name\": \"preview_site\", \"arguments\": {{\"path\": {folder:?}}}}}.)"
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// For a model that stopped with problems the preview found: one call that
/// starts fixing them (it carries on from there by itself).
pub fn forced_fix(tools: &Value, problems: &str) -> Option<ForcedLookup> {
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let list: Vec<&Value> = tools.as_array()?.iter().filter(|t| matches!(name_of(t).as_str(), "edit_file" | "write_file" | "read_file")).collect();
    if list.is_empty() {
        return None;
    }
    let names: Vec<String> = list.iter().map(|t| name_of(t)).collect();
    let schema = json!({ "oneOf": list.iter().map(|t| json!({
        "type": "object",
        "properties": { "name": { "const": name_of(t) }, "arguments": t["function"]["parameters"] },
        "required": ["name", "arguments"],
    })).collect::<Vec<_>>() });
    Some(ForcedLookup {
        names,
        nudge: format!(
            "(The site isn't finished: the preview found these problems.\n{problems}\nDon't stop or tell me it's done: fix them now. \
             Read the file first if you need to, then change it. Reply with only one tool call as JSON, {{\"name\": ..., \"arguments\": {{...}}}}.)"
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// A site's stylesheet or script written but no page to load them: the
/// folder its index.html belongs in, and the files it should link.
pub fn page_missing(written: &[std::path::PathBuf]) -> Option<(std::path::PathBuf, Vec<String>)> {
    let ext = |p: &std::path::PathBuf| p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if written.iter().any(|p| matches!(ext(p).as_str(), "html" | "htm")) {
        return None;
    }
    let parts: Vec<&std::path::PathBuf> = written.iter().filter(|p| matches!(ext(p).as_str(), "css" | "js" | "mjs")).collect();
    let first = parts.first()?;
    // A script in js/ or css/ belongs to the folder above.
    let mut dir = first.parent()?.to_path_buf();
    if dir.file_name().is_some_and(|n| matches!(n.to_string_lossy().to_ascii_lowercase().as_str(), "js" | "css" | "scripts" | "styles" | "assets")) {
        dir = dir.parent()?.to_path_buf();
    }
    let index = dir.join("index.html");
    if index.exists() {
        return None;
    }
    let links = parts.iter().filter_map(|p| p.strip_prefix(&dir).ok()).map(|p| p.to_string_lossy().replace('\\', "/")).collect();
    Some((dir, links))
}

/// One call that writes the page a site's stylesheet and script were made for.
pub fn forced_page(tools: &Value, index: &std::path::Path, links: &[String]) -> Option<ForcedLookup> {
    let mut f = forced_write(tools, "index.html", index)?;
    f.nudge = format!(
        "(The site has {} but no page: write index.html now with write_file (path {:?}), the whole page, linking {} and with every part the user asked for. Reply with only one tool call as JSON, {{\"name\": \"write_file\", \"arguments\": {{...}}}}.)",
        links.join(" and "),
        index.display().to_string(),
        links.join(" and ")
    );
    Some(f)
}

/// The folder of the newest web page written (where a site's index.html is).
pub fn site_folder(written: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    written
        .iter()
        .rev()
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm")))
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
}

/// For a model that pasted what it was asked to make: one call that makes it
/// on the PC (the first file, or asking for a folder to put it in).
pub fn forced_build(tools: &Value) -> Option<ForcedLookup> {
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let all = tools.as_array()?;
    let has = |n: &str| all.iter().any(|t| name_of(t) == n);
    // A file, not just a folder (a small model would stop there).
    let pick: &[&str] = if has("write_file") { &["write_file"] } else if has("request_folder") { &["request_folder"] } else { return None };
    let list: Vec<&Value> = all.iter().filter(|t| pick.contains(&name_of(t).as_str())).collect();
    let names: Vec<String> = list.iter().map(|t| name_of(t)).collect();
    let schema = json!({ "oneOf": list.iter().map(|t| json!({
        "type": "object",
        "properties": { "name": { "const": name_of(t) }, "arguments": t["function"]["parameters"] },
        "required": ["name", "arguments"],
    })).collect::<Vec<_>>() });
    let what = if names[0] == "write_file" {
        "Make a sensible first version on my PC now, choosing the details yourself: write the first file with write_file \
         (its full, working contents, not a sketch; for a website, one index.html with its CSS and JavaScript inside is fine). \
         You'll write any other files next."
    } else {
        "Make it on my PC: first ask for a folder to work in with request_folder."
    };
    Some(ForcedLookup {
        names,
        nudge: format!("(Don't paste the code or give me steps to follow. {what} Reply with only one tool call as JSON, {{\"name\": ..., \"arguments\": {{...}}}}.)"),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
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
        ("find_places", &["near me", "nearby", "nearest", "closest", "around here", "near "]),
        ("search_meetings", &["meeting notes", "transcript"]),
        ("market_quote", &["stock", "share price", "price of", "crypto", "bitcoin", "exchange rate", "currency"]),
        ("research", &["research", "compare", "better", "best", "versus", " vs", "pros and cons"]),
        ("wikipedia", &["who is", "who was", "what is", "tell me about"]),
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
pub(crate) fn salvage(text: &str, offered: &[String]) -> (Vec<ToolCall>, String) {
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
        // Background work doesn't need step-by-step thinking: Qwen-style
        // models turn it off, gpt-oss keeps it short (models without these
        // switches ignore them).
        "chat_template_kwargs": { "enable_thinking": false, "reasoning_effort": "low" },
    });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    let v = complete_once(ep, &body).await?;
    let content = v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or_default();
    let answer = strip_thinking(content).trim();
    // A model that thinks anyway can spend the whole allowance thinking and
    // never answer; once, give it room to finish.
    let thought = v.pointer("/choices/0/message/reasoning_content").and_then(Value::as_str).is_some_and(|r| !r.trim().is_empty())
        || content.trim_start().starts_with("<think>");
    if answer.is_empty() && thought && v.pointer("/choices/0/finish_reason").and_then(Value::as_str) == Some("length") {
        body["max_tokens"] = json!(max_tokens + 768);
        let v = complete_once(ep, &body).await?;
        let content = v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or_default();
        return Ok(strip_thinking(content).trim().to_string());
    }
    Ok(answer.to_string())
}

async fn complete_once(ep: &Endpoint, body: &Value) -> Result<Value, String> {
    let resp = net::local_client()
        .post(ep.url("/v1/chat/completions"))
        .bearer_auth(&ep.key)
        .json(body)
        // A model held to part of the processor (GPU limit) is slow.
        .timeout(Duration::from_secs(1800))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the engine: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| format!("The engine sent an unreadable answer: {e}"))?;
    if !status.is_success() {
        let detail = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
        return Err(format!("The engine returned {status}: {detail}"));
    }
    Ok(v)
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
    let raw = complete(ep, messages, json!({}), 48).await.ok()?;
    clean_title(&raw)
}

fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_start_matches(|c: char| c == '#' || c == '*' || c.is_whitespace());
    let line = line.strip_prefix("Title:").unwrap_or(line).trim();
    let t: String = line.trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '*' | '.' | '`')).trim().chars().take(60).collect();
    (!t.is_empty() && t.split_whitespace().count() <= 10).then_some(t)
}

/// The chat's name until the model names it (or if it can't): the first
/// sentence of the request, without greetings and "can you", cut at a word.
pub fn title_from(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("New chat");
    // The first sentence.
    let mut s = line;
    if let Some(i) = s.char_indices().find(|&(i, c)| matches!(c, '.' | '?' | '!') && s[i + c.len_utf8()..].starts_with(' ')).map(|(i, _)| i) {
        s = &s[..i];
    }
    // Greetings and polite openers say nothing about the chat.
    const OPENERS: [&str; 16] = [
        "hey", "hi", "hello", "ok", "okay", "so", "please", "can you", "could you", "would you", "will you", "i want you to", "i need you to", "i'd like you to", "help me", "i need help",
    ];
    let skip = |t: &str| t.trim_start_matches(|c: char| c == ',' || c.is_whitespace()).to_string();
    let mut rest = skip(s);
    while let Some(o) = OPENERS.iter().find(|o| {
        let lower = rest.to_lowercase();
        lower.starts_with(*o) && lower[o.len()..].chars().next().is_none_or(|c| c == ' ' || c == ',')
    }) {
        rest = skip(&rest[o.len()..]);
    }
    let s = if rest.split_whitespace().count() >= 2 { rest.as_str() } else { s };
    // Up to 48 characters, ending at a whole word.
    let mut title = String::new();
    for w in s.split_whitespace() {
        if title.chars().count() + w.chars().count() + 1 > 48 {
            if title.is_empty() {
                title = w.chars().take(48).collect();
            }
            title.push('…');
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(w);
    }
    let title = title.trim_end_matches([',', ':', ';']).to_string();
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "New chat".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gpt-oss's Harmony tool calls that reach the reply as text are run,
    /// not shown to the user; its built-in tools become ours.
    #[test]
    fn harmony_tool_calls_written_as_text_are_run() {
        let offered: Vec<String> = ["find_files", "run_command", "fetch_page"].map(String::from).to_vec();
        let raw = "<|channel|>commentary to=functions.find_files <|constrain|>json<|message|>{\"pattern\": \"*hello*\"}<|call|>";
        let (calls, rest) = harmony_calls(raw, &offered);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "find_files");
        assert_eq!(serde_json::from_str::<Value>(&calls[0].arguments).unwrap()["pattern"], "*hello*");
        assert!(rest.is_empty(), "{rest:?}");
        // The special tokens left out, and words around the call kept.
        let (calls, rest) = harmony_calls("Let me look. commentary to=functions.run_command json{\"command\": \"dir\"}", &offered);
        assert_eq!(calls[0].name, "run_command");
        assert_eq!(rest, "Let me look.");
        // Built-in tools as ours.
        let (calls, _) = harmony_calls("to=browser.open <|message|>{\"url\":\"https://example.com\"}", &offered);
        assert_eq!(calls[0].name, "fetch_page");
        assert_eq!(harmony_alias("python", &json!({ "code": "print(1)" })).unwrap().0, "run_code");
        // Not calls: an unknown tool, or other words between the name and the JSON.
        assert!(harmony_calls("to=functions.launch_rockets json{\"x\":1}", &offered).0.is_empty());
        assert!(harmony_calls("I went to=functions.find_files and then {\"a\":1}", &offered).0.is_empty());
        // Braces inside strings don't end the arguments.
        let (calls, _) = harmony_calls("to=functions.run_command<|message|>{\"command\": \"echo }{\"}", &offered);
        assert_eq!(serde_json::from_str::<Value>(&calls[0].arguments).unwrap()["command"], "echo }{");
    }

    #[test]
    fn explaining_instead_of_doing_is_noticed() {
        assert!(asks_to_act("Fetch this page and apply its instructions to my project folder"));
        assert!(asks_to_act("find hello.bat and run it"));
        assert!(!asks_to_act("What is the capital of France?"));
        assert!(explains_instead("Here are the steps:\n1. Open a terminal\n2. Run npm install"));
        assert!(explains_instead("Would you like me to go ahead and run it?"));
        assert!(explains_instead("You'll need to run the installer first."));
        assert!(!explains_instead("Done. I created notes.txt and ran the script; it printed 42."));
    }

    fn m(role: &str, id: &str) -> Message {
        Message { id: id.into(), role: role.into(), content: id.into(), ..Default::default() }
    }

    #[test]
    fn a_fix_pasted_into_the_reply_is_saved_next() {
        let code: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let reply = format!("Great! Now I'll update the `app.js` file.\n\n```javascript\n{code}```");
        assert!(pasted_code(&reply) && !pasted_code("Here's a short one:\n```js\nlet a = 1;\n```"));
        let written = vec![std::path::PathBuf::from("C:/s/index.html"), std::path::PathBuf::from("C:/s/app.js"), std::path::PathBuf::from("C:/s/style.css")];
        assert_eq!(named_file(&reply, &written), Some(std::path::PathBuf::from("C:/s/app.js")));
        assert_eq!(named_file("fixed it", &written), Some(std::path::PathBuf::from("C:/s/app.js")), "the newest script");
        let tools = json!([{ "type": "function", "function": { "name": "write_file", "description": "", "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } } } } }]);
        let f = forced_save(&tools, std::path::Path::new("live-test2/app.js")).unwrap();
        assert!(f.nudge.contains("never saved"));
        assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["arguments"]["properties"]["path"]["const"], "live-test2/app.js");
    }

    #[test]
    fn asking_where_they_are_for_a_near_me_question_is_a_stall() {
        let offered = vec!["find_places".to_string()];
        assert!(stalls("Find coffee shops near me.", "To find coffee shops near you, I'll need your location. Could you please provide the city or address where you are?", &offered));
        let tools = json!([
            { "type": "function", "function": { "name": "web_search", "description": "", "parameters": { "type": "object" } } },
            { "type": "function", "function": { "name": "find_places", "description": "", "parameters": { "type": "object" } } },
        ]);
        assert_eq!(forced_lookup(&tools, "Find coffee shops near me.", |_| true).unwrap().names, vec!["find_places".to_string()]);
    }

    #[test]
    fn announcing_a_lookup_without_doing_it_is_a_stall() {
        let offered = vec!["research".to_string(), "web_search".to_string()];
        let said = "To research which is better, I'll perform a few web searches. Here are the searches I'll run:\n\n1. \"iPhone 17 battery life\"\n2. \"Pixel 10 battery life\"\n\nLet's execute these searches and gather the results.";
        assert!(stalls("Research which is better for battery life", said, &offered));
        // An answer that mentions looking things up along the way isn't one.
        assert!(!stalls("Who is Link?", "Link is the hero of The Legend of Zelda. Let me know if you'd like more details!", &offered));
        // A tool's name in code it wrote isn't telling the user to use it.
        assert!(!stalls("Write a search helper", "Here it is:\n```js\nfunction web_search(q) { return fetch(q); }\n```\nCall `web_search` with your query.", &offered));
        assert!(stalls("Find the news", "You can use web_search to find it.", &offered));
        let tools = json!([
            { "type": "function", "function": { "name": "web_search", "description": "", "parameters": { "type": "object" } } },
            { "type": "function", "function": { "name": "research", "description": "", "parameters": { "type": "object" } } },
        ]);
        let f = forced_lookup(&tools, "Research which is better, the iPhone 17 or the Pixel 10", |_| true).unwrap();
        assert_eq!(f.names, vec!["research".to_string()]);
    }

    #[test]
    fn sums_are_worked_out_not_guessed() {
        for ask in [
            "what's 37*48?",
            "What is 15% of 2,345",
            "how much is 1250 divided by 7",
            "1234 + 5678",
            "what is 2^10",
            "How many days until Christmas?",
            "what's the monthly payment on a 300k mortgage at 6.5% for 30 years",
            "split $184.50 4 ways",
            "square root of 1764",
            "Solve x^2 - 5x + 6 = 0 and show the steps.",
            "What's the integral of x^2 from 0 to 3?",
            "find the derivative of sin(x) at 1",
        ] {
            assert!(asks_math(ask), "{ask}");
        }
        for not in ["what's on 10/17?", "make a 3x3 grid game", "my birthday is 2026-10-17", "Build me a calculator app", "tell me about Apollo 11", "I have 2 cats"] {
            assert!(!asks_math(not), "{not}");
        }
        let tools = json!([{ "type": "function", "function": { "name": "calculate", "description": "", "parameters": { "type": "object" } } }]);
        let f = forced_calc(&tools).unwrap();
        assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["name"]["const"], "calculate");
        assert!(forced_calc(&json!([])).is_none());
    }

    /// gpt-oss thinks even when asked not to: with a small allowance it ran
    /// out before answering, so chats kept the raw request as their name.
    #[tokio::test]
    async fn a_model_that_spends_its_allowance_thinking_gets_room_to_answer() {
        use axum::{routing::post, Json, Router};
        use std::sync::atomic::{AtomicU64, Ordering};
        static LAST_MAX: AtomicU64 = AtomicU64::new(0);
        async fn reply(Json(body): Json<Value>) -> Json<Value> {
            let max = body["max_tokens"].as_u64().unwrap_or(0);
            LAST_MAX.store(max, Ordering::SeqCst);
            assert_eq!(body["chat_template_kwargs"]["reasoning_effort"], "low");
            Json(if max < 100 {
                json!({ "choices": [{ "finish_reason": "length", "message": { "content": "", "reasoning_content": "The user wants a title. Let me think about" } }] })
            } else {
                json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "Summarize Latest Google Email", "reasoning_content": "Short title." } }] })
            })
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, Router::new().route("/v1/chat/completions", post(reply))).await.unwrap() });
        let ep = Endpoint { port, key: "k".into(), ctx: 8192, extra: json!({}), cloud: None };
        assert_eq!(summary_title(&ep, "Summarize my most recent email from Google").await.as_deref(), Some("Summarize Latest Google Email"));
        assert_eq!(LAST_MAX.load(Ordering::SeqCst), 48 + 768, "asked again with room");
    }

    #[test]
    fn replies_that_dont_know_the_thing_are_caught() {
        for r in [
            "I'm not familiar with \"Skibidi Fortnite Rizz\". Could you tell me more?",
            "I'm not sure what Sulcus Studio is, but here's a guess.",
            "As of my knowledge cutoff in 2023, there is no game called that.",
            "I don't know what the Lumen Loop refers to.",
            "Could you clarify what a bloopsnorf is?",
            "I couldn't find information about that band.",
        ] {
            assert!(doesnt_know(r), "{r}");
        }
        for r in [
            "Here's a picture of a castle at sunset.",
            "I'm not sure what style you'd like, so I made it bright and colorful.",
            "I don't know your schedule yet, so let me check it.",
            "The Legend of Zelda is an action adventure series by Nintendo.",
        ] {
            assert!(!doesnt_know(r), "{r}");
        }
        let tools = json!([
            { "type": "function", "function": { "name": "request_web", "description": "", "parameters": { "type": "object" } } },
            { "type": "function", "function": { "name": "search_notes", "description": "", "parameters": { "type": "object" } } },
        ]);
        let f = forced_web(&tools).unwrap();
        assert_eq!(f.names, vec!["request_web".to_string()]);
        assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["name"]["const"], "request_web");
        assert!(forced_web(&json!([{ "type": "function", "function": { "name": "search_notes", "parameters": {} } }])).is_none());
    }

    #[test]
    fn files_it_already_wrote_are_left_out_of_what_the_model_is_sent() {
        let page = format!("<html>{}</html>", "<p>row</p>\n".repeat(80));
        let pasted = format!("Here it is:\n```html\n{page}\n```\nNow I'll save it.");
        let call = |name: &str, args: Value| ToolCall { id: "c".into(), name: name.into(), arguments: args.to_string() };
        let history = vec![
            Message { role: "user".into(), content: "Build a site".into(), ..Default::default() },
            Message { role: "assistant".into(), content: pasted.clone(), ..Default::default() },
            Message { role: "assistant".into(), tool_calls: Some(vec![call("write_file", json!({ "path": "s/index.html", "content": page }))]), ..Default::default() },
            Message { role: "tool".into(), content: "Created s/index.html".into(), tool_call_id: Some("c".into()), ..Default::default() },
            Message { role: "assistant".into(), tool_calls: Some(vec![call("write_file", json!({ "path": "s/app.js", "content": "x".repeat(900) }))]), ..Default::default() },
        ];
        let slim = without_written(&history);
        assert!(!slim[1].content.contains("<p>row</p>") && !slim[1].content.contains('[') && slim[1].content.starts_with("Here it is:"), "{}", slim[1].content);
        let args: Value = serde_json::from_str(&slim[2].tool_calls.as_ref().unwrap()[0].arguments).unwrap();
        assert!(args.get("content").is_none(), "no stand-in for the model to copy: {args}");
        assert_eq!(args["path"], "s/index.html");
        // The newest message is sent whole.
        assert_eq!(slim[4].tool_calls, history[4].tool_calls);
        assert_eq!(slim[0].content, "Build a site");
    }

    #[test]
    fn a_built_site_is_previewed_and_its_problems_fixed() {
        let def = |n: &str| json!({ "type": "function", "function": { "name": n, "description": "", "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "port": { "type": "integer" } } } } });
        let tools = json!([def("preview_site"), def("edit_file"), def("write_file"), def("run_command")]);
        let p = forced_preview(&tools, "stock-site/").unwrap();
        let args = &p.response_format["json_schema"]["schema"]["properties"]["arguments"];
        assert_eq!(args["properties"]["path"]["const"], "stock-site/");
        assert!(args["properties"].get("port").is_none());
        let f = forced_fix(&tools, "- Script error: x is not defined").unwrap();
        assert_eq!(f.names, vec!["edit_file".to_string(), "write_file".to_string()]);
        assert!(f.nudge.contains("x is not defined"));
        assert!(forced_preview(&json!([def("write_file")]), "a/").is_none());
        let written = vec![std::path::PathBuf::from("C:/s/site/app.js"), std::path::PathBuf::from("C:/s/site/index.html"), std::path::PathBuf::from("C:/s/site/style.css")];
        assert_eq!(site_folder(&written), Some(std::path::PathBuf::from("C:/s/site")));
        assert_eq!(site_folder(&written[..1]), None);
        // A stylesheet and script but no page (what Qwen2.5-Coder 14B left in the live test).
        let base = std::env::temp_dir().join(format!("sulcus-page-{}", uuid::Uuid::new_v4().simple()));
        let parts = vec![base.join("style.css"), base.join("js").join("app.js")];
        let (dir, links) = page_missing(&parts).unwrap();
        assert_eq!(dir, base);
        assert_eq!(links, vec!["style.css".to_string(), "js/app.js".to_string()]);
        assert!(page_missing(&written).is_none(), "a page was written");
        let tools = json!([def("write_file")]);
        let f = forced_page(&tools, &dir.join("index.html"), &links).unwrap();
        assert!(f.nudge.contains("style.css and js/app.js"));
        assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["arguments"]["properties"]["path"]["const"], dir.join("index.html").display().to_string());
    }

    #[test]
    fn picture_requests_get_a_picture_even_when_the_model_says_it_cant() {
        for a in ["Make me a picture of Link from The Legend of Zelda in pixel art.", "can you draw a cat wearing a hat", "I want a logo for my bakery", "image of a sunset over Provo"] {
            assert!(asks_for_picture(a), "{a}");
        }
        for a in ["What's in this picture?", "Build me a stock exchange website", "how do I resize an image in Photoshop"] {
            assert!(!asks_for_picture(a), "{a}");
        }
        assert!(denies_pictures("I'm sorry, but I can't generate images directly. However, I can help you find templates."));
        assert!(denies_pictures("As a text model I am unable to create pictures."));
        assert!(!denies_pictures("Here's Link in pixel art, standing on a grassy hill."));
        let tools = json!([{ "type": "function", "function": { "name": "create_image", "description": "", "parameters": { "type": "object", "properties": { "prompt": { "type": "string" } } } } }]);
        assert_eq!(forced_picture(&tools, false).unwrap().names, vec!["create_image".to_string()]);
        assert!(forced_picture(&json!([]), false).is_none());
        let both = json!([
            { "type": "function", "function": { "name": "create_image", "description": "", "parameters": { "type": "object", "properties": { "prompt": { "type": "string" } } } } },
            { "type": "function", "function": { "name": "edit_image", "description": "", "parameters": { "type": "object", "properties": { "instruction": { "type": "string" } } } } }
        ]);
        assert_eq!(forced_picture(&both, true).unwrap().names, vec!["edit_image".to_string()]);
        assert_eq!(forced_picture(&tools, true).unwrap().names, vec!["create_image".to_string()], "no edit tool: make one instead");
        assert!(asks_picture_change("can you add a pirate hat to me"));
        assert!(asks_picture_change("Make me look like a cartoon"));
        assert!(asks_picture_change("remove the car in the back"));
        assert!(!asks_picture_change("what is in this picture?"));
        assert!(!wants_several("Make me a picture of Link from The Legend of Zelda in pixel art."));
        assert!(wants_several("make 3 pictures of a cat") && wants_several("show me a few versions of the logo"));
    }

    #[test]
    fn named_things_get_their_look_spelled_out_for_the_picture_model() {
        let names = names_of(r#"{"names":[
            {"name":"Link","known":true,"looks":"elf-eared young hero in a green tunic and pointed green cap, blond hair, sword and blue-red shield"},
            {"name":"Hyrule","known":false,"looks":""},
            {"name":"link","known":true,"looks":"duplicate"},
            {"name":"","known":true,"looks":"no name"}]}"#);
        assert_eq!(names.len(), 2, "duplicates and blanks dropped: {names:?}");
        assert!(names[0].described() && !names[1].described());
        let p = with_looks("Pixel art of Link from the Legend of Zelda on a hill.", &names);
        assert_eq!(p, "Pixel art of Link from the Legend of Zelda on a hill. Link looks like this: elf-eared young hero in a green tunic and pointed green cap, blond hair, sword and blue-red shield.");
        assert_eq!(with_looks("A dog on a beach", &[]), "A dog on a beach");
        assert!(names_of("not json").is_empty());
        assert_eq!(looks_of(r#"{"looks":" green tunic, pointed cap "}"#), "green tunic, pointed cap");
        // "him" isn't a name; "Link" is, as written; "zelda" in lowercase only isn't.
        let listed = names_of(r#"{"names":[{"name":"him","known":false,"looks":""},{"name":"HIM","known":true,"looks":"lobster claws, red skin, black lips, pink tulle"},{"name":"Link","known":true,"looks":"green tunic and cap, pointy ears, blond"},{"name":"Zelda","known":false,"looks":""}]}"#);
        let kept = real_names(listed, "Now put him at a party with Link", "a party with balloons, the man smiling, Link beside him");
        assert_eq!(kept.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["Link"]);
        let (ask, format) = names_check("draw link", "Link on a hill");
        assert!(ask.contains("draw link") && ask.contains("Link on a hill"));
        assert_eq!(format["json_schema"]["name"], "names");
    }

    #[test]
    fn title_uses_first_nonempty_line_and_truncates() {
        assert_eq!(title_from("\n  hello there \nmore"), "Hello there");
        assert_eq!(title_from("Hey, can you summarize my most recent email from Google? Thanks"), "Summarize my most recent email from Google");
        assert_eq!(title_from("please help me plan a three day trip to Zion National Park with my family in November"), "Plan a three day trip to Zion National Park with…");
        assert_eq!(title_from("what's the weather in Orem"), "What's the weather in Orem");
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
    fn notices_a_model_pasting_what_it_was_asked_to_make() {
        let files = vec!["read_file".to_string(), "write_file".to_string()];
        let code = "Sure! Here's a simple site.\n\n```html\n<!doctype html>\n<html>\n<body>\n<h1>Hi</h1>\n</body>\n</html>\n```\nSave it as index.html and open it.";
        assert!(builds_in_chat(&["Make me a website for my bakery"], code, &files));
        assert!(builds_in_chat(&["can you create a python script that renames my photos"], "Step 1: open your terminal. Step 2: ...", &files));
        assert!(builds_in_chat(&["put this in notes.txt"], "Create a new file called notes.txt and paste this in.", &files));
        // No folder yet: asking for one still counts as making it.
        assert!(builds_in_chat(&["Build a todo app"], code, &["request_folder".to_string()]));
        // Nothing to make it with, a question, or a short snippet in an explanation: left alone.
        assert!(!builds_in_chat(&["Make me a website"], code, &["read_file".to_string()]));
        assert!(!builds_in_chat(&["What's a closure in Rust?"], "```rust\nlet f = |x| x + 1;\nprintln!(\"{}\", f(1));\nlet g = f;\n```", &files));
        assert!(!builds_in_chat(&["How do I write a for loop in Python?"], "Like this: `for x in xs:`", &files));
        // What his qwen2.5-coder said after "try again to do it for me" (the build ask came earlier).
        let refusal = "I'm sorry for the confusion. As an AI, I can't directly execute commands on your computer to build a website. However, I can guide you through the process step-by-step.";
        assert!(builds_in_chat(&["try again to do it for me", "Do all this for me", "Build the website and run it"], refusal, &files));
        assert!(!builds_in_chat(&["try again to do it for me", "What's the weather like?"], refusal, &files), "nothing was asked to be made");
        // A page that uses files it didn't write: they're named for writing next.
        let dir = std::env::temp_dir().join(format!("sulcus-pieces-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("index.html");
        std::fs::write(&page, r#"<link rel="stylesheet" href="styles.css"><script src="script.js"></script><script src="https://cdn.example.com/x.js"></script>"#).unwrap();
        std::fs::write(dir.join("script.js"), "1").unwrap();
        let missing = missing_pieces(&[page.clone()]);
        assert_eq!(missing, vec![("index.html".to_string(), dir.join("styles.css"))], "styles.css is missing; script.js is there; the CDN one isn't ours");
        let tools = json!([{ "type": "function", "function": { "name": "write_file", "description": "", "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } } } } }]);
        let f = forced_write(&tools, "index.html", &dir.join("styles.css")).unwrap();
        assert!(f.nudge.contains("styles.css") && f.response_format.to_string().contains("const"));
        std::fs::remove_dir_all(&dir).ok();
        // What his qwen2.5-coder 1.5B said to "Build me a live stock exchange website".
        assert!(builds_in_chat(&["Build me a live stock exchange website"], "Sure, I can help you build a live stock exchange website. What kind of features do you need?", &files));
        assert!(!builds_in_chat(&["What's a good name for a bakery?"], "How about Sweet Crumbs? Would you like more ideas?", &files));
        // Documents go to create_document; sites and code don't.
        assert!(wants_a_document("Write a report as a Word doc") && wants_a_document("make slides about Q3") && !wants_a_document("Build me a live stock exchange website"));
        let forced = forced_build(&json!([{ "type": "function", "function": { "name": "write_file", "description": "", "parameters": { "type": "object" } } }, { "type": "function", "function": { "name": "read_file", "description": "", "parameters": { "type": "object" } } }])).unwrap();
        assert_eq!(forced.names, vec!["write_file"]);
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

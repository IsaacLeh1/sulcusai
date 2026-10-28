// SPDX-License-Identifier: AGPL-3.0-only
//! Builds the prompt (profile + history within the context window) and
//! streams the model's reply, including any tool calls it makes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::{Message, Profile, ToolCall};
use crate::engine::Endpoint;
use crate::net;

mod asks;
mod calls;
mod follow_ups;
#[cfg(test)]
mod tests;

pub use asks::*;
pub(crate) use calls::*;
pub use follow_ups::*;

/// Tokens each message adds for its role markers in the chat template.
const PER_MESSAGE_OVERHEAD: u32 = 6;
const TRIMMED_TOOL_OUTPUT: &str = "[Earlier tool output removed to make room. Run the tool again if you need it.]";

pub fn system_prompt(profile: &Profile, today: &str) -> (String, String) {
    let base = format!(
        "You are SulcusAI, a helpful assistant. Be clear, accurate and friendly. \
         Use Markdown when it helps; write math formulas in LaTeX, $...$ in a sentence and $$...$$ on their own lines. \
         If you don't know something, say so. What you work out while thinking goes in your answer itself \
         (names, links, numbers, steps): the answer is what the user reads. Think to work things out, not to draft \
         the reply: write the answer once.\n\n\
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    /// Worked out from the saved messages, not counted by the engine (a chat
    /// not used since this update).
    #[serde(default)]
    pub estimated: bool,
    /// What the conversation's share is made of.
    #[serde(default)]
    pub history: HistoryParts,
}

/// The conversation in the window, by kind (tokens).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct HistoryParts {
    /// The person's messages.
    pub yours: u32,
    /// The assistant's replies, with the tool calls it wrote.
    pub replies: u32,
    /// What tools found (files read, pages, searches).
    pub tool_results: u32,
    /// The summary an earlier part was compacted into.
    pub summary: u32,
}

impl HistoryParts {
    /// Adds one message's tokens to its kind.
    pub fn add(&mut self, m: &Message, tokens: u32) {
        let slot = match m.role.as_str() {
            _ if m.meta.as_ref().is_some_and(|v| v.get("compacted").is_some()) => &mut self.summary,
            "user" => &mut self.yours,
            "tool" => &mut self.tool_results,
            _ => &mut self.replies,
        };
        *slot += tokens;
    }
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

/// Past 60% of the model's memory, long tool results older than the last
/// six are swapped for the stub (past 75%, older than the last three). It
/// happens in batches of six results, not one more every step, so the
/// start of each request stays the same for a while: a provider's prompt
/// cache and the engine's reuse both need that.
pub fn trim_in_steps(history: &[Message], ctx: u32) -> Vec<Message> {
    // About 3 characters a token, as elsewhere here.
    let tokens: usize = history.iter().map(|m| (m.content.len() + m.tool_calls.as_ref().map_or(0, |c| c.iter().map(|c| c.arguments.len()).sum())) / 3).sum();
    let share = tokens as f64 / ctx.max(1) as f64;
    let keep = match share {
        s if s >= 0.75 => 3,
        s if s >= 0.6 => 6,
        _ => return history.to_vec(),
    };
    let results: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == "tool").map(|(i, _)| i).collect();
    let old = results.len().saturating_sub(keep) / 6 * 6;
    let mut out = history.to_vec();
    for &i in &results[..old] {
        if out[i].content.chars().count() > 2_000 {
            out[i].content = TRIMMED_TOOL_OUTPUT.into();
        }
    }
    out
}

/// The request didn't fit the model's memory: this PC's engine, Anthropic
/// ("prompt is too long") or an OpenAI-style provider saying so.
pub fn too_long(e: &str) -> bool {
    let l = e.to_lowercase();
    ["no longer fits in the model's memory", "prompt is too long", "context_length_exceeded", "maximum context length"].iter().any(|s| l.contains(s))
}

/// Every tool result of this turn (from the message `turn_id`) swapped for
/// the stub but the last `keep`.
pub fn trim_turn(kept: &mut [Message], turn_id: &str, keep: usize) {
    let Some(start) = kept.iter().position(|m| m.id == turn_id) else { return };
    let results: Vec<usize> = (start..kept.len()).filter(|&i| kept[i].role == "tool").collect();
    for &i in &results[..results.len().saturating_sub(keep)] {
        kept[i].content = TRIMMED_TOOL_OUTPUT.into();
    }
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

    let history = trim_in_steps(history, ep.ctx);
    let groups = turns(&history);
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
    let mut parts = HistoryParts::default();
    for m in &kept {
        parts.add(m, message_tokens(ep, m).await?);
    }
    let info = ContextInfo {
        ctx: ep.ctx,
        system_tokens,
        profile_tokens,
        tools_tokens,
        history_tokens: used,
        reply_reserve,
        dropped_messages: history.len().saturating_sub(kept.len()),
        last_total: None,
        estimated: false,
        history: parts,
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
                    .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": whole_arguments(&c.arguments) } }))
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
/// Adds a line to a message's text, whether it's plain text or parts (text and pictures).
pub fn add_text(slot: &mut Value, note: &str) {
    match &mut slot["content"] {
        Value::String(s) => {
            s.push('\n');
            s.push_str(note);
        }
        Value::Array(parts) => parts.push(json!({ "type": "text", "text": note })),
        other => *other = json!(note),
    }
}

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

#[derive(Default)]
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
    /// Input tokens read from the provider's prompt cache (cheaper), and
    /// written to it (a little dearer), when it says.
    pub cache_read: Option<u32>,
    pub cache_write: Option<u32>,
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
    let (mut first, cut_short, at_length) = stream_local(ep, messages.clone(), tools, cancel, &mut on_delta).await?;
    let unanswered = first.content.trim().is_empty() && first.tool_calls.is_empty() && !first.thinking.trim().is_empty() && !first.cancelled;
    // It ended its reply inside its thinking (some models write the whole
    // answer there, then stop): that's the answer. Asked to write it again,
    // a 30B Nemotron answered twice, the second in another format.
    if unanswered && !cut_short {
        first.content = std::mem::take(&mut first.thinking).trim().to_string();
        return Ok(first);
    }
    // A thinking model whose thinking was cut short (its allowance ran out,
    // or it got stuck repeating itself): once more, without thinking, given
    // what it thought. Started over, it answered without what the user had
    // watched it find.
    if unanswered {
        let mut quick = ep.clone();
        // gpt-oss ignores enable_thinking and reads its effort from the
        // template's settings: "low" there too, or the retry repeated itself.
        quick.extra["chat_template_kwargs"] = json!({ "enable_thinking": false, "reasoning_effort": "low" });
        quick.extra["reasoning_effort"] = json!("low");
        let mut messages = messages;
        let note = format!(
            "[Your thinking so far, which stopped before you answered:]\n{}\n[Now write your answer to the user from it, without thinking again. \
             Put what you found in it (names, links, numbers, steps): the user reads your answer, not your thinking.]",
            thought_so_far(&first.thinking, ep.ctx as usize)
        );
        match messages.last_mut().filter(|m| m["role"] == "user") {
            Some(last) => add_text(last, &format!("\n{note}")),
            None => messages.push(json!({ "role": "user", "content": note })),
        }
        let (mut again, _, again_at_length) = stream_local(&quick, messages.clone(), tools, cancel, &mut on_delta).await?;
        again.thinking = format!("{}{}", first.thinking, again.thinking);
        if again.content.trim().is_empty() && again.tool_calls.is_empty() && !again.cancelled {
            return Err(THOUGHT_ONLY.into());
        }
        if again_at_length {
            go_on_past_the_limit(&quick, &messages, &mut again, cancel, &mut on_delta).await?;
        }
        return Ok(again);
    }
    // A tool call cut off partway (a long file or note): once more with
    // twice the room, showing only what wasn't shown already; else the call
    // is dropped, so a broken one isn't run or saved.
    if at_length && cut_call(&first) {
        let mut roomier = ep.clone();
        let limit = ep.extra.get("max_tokens").and_then(Value::as_u64).unwrap_or((ep.ctx / 2).clamp(512, 4096) as u64);
        roomier.extra["max_tokens"] = json!((limit * 2).min(ep.ctx as u64 * 3 / 4).max(limit));
        let shown = first.content.len();
        let mut seen = 0usize;
        let mut quiet = |d: Delta| {
            if let Delta::Content(t) = &d {
                let from = shown.saturating_sub(seen).min(t.len());
                seen += t.len();
                if from < t.len() && t.is_char_boundary(from) {
                    on_delta(Delta::Content(t[from..].to_string()));
                }
            }
        };
        let (again, _, still) = stream_local(&roomier, messages.clone(), tools, cancel, &mut quiet).await?;
        crate::applog::line("chat", &format!("A tool call was cut off at the length limit; tried again with more room ({})", if still { "cut again" } else { "whole" }));
        first = again;
        first.tool_calls.retain(|c| c.arguments.trim().is_empty() || serde_json::from_str::<Value>(&c.arguments).is_ok());
        return Ok(first);
    }
    if at_length {
        go_on_past_the_limit(ep, &messages, &mut first, cancel, &mut on_delta).await?;
    }
    Ok(first)
}

/// A reply and what it went on with after its length limit, joined where it
/// really goes on. Models carrying on often go back first: to the line they
/// were cut in ("…and eyes" then "With your fluffy fur and eyes so deep"), or
/// to the start of the section ("[Verse 1]" again); that's written once. And
/// a line cut at its end goes on with the next on its own line.
pub fn join_continued(done: &str, rest: &str) -> String {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let lines: Vec<(usize, &str)> = done.split_inclusive('\n').scan(0, |at, l| {
        let start = *at;
        *at += l.len();
        Some((start, l.trim_end_matches(['\n', '\r'])))
    }).collect();
    let first = rest.trim_start().lines().next().unwrap_or("");
    let nf = norm(first);
    if nf.len() >= 4 && !lines.is_empty() {
        // The section it was cut in: from its heading or the blank line before it.
        let heading = |l: &str| {
            let t = l.trim().trim_matches(['*', '#', ' ']);
            t.starts_with('[') && t.ends_with(']')
        };
        let from = lines.iter().rposition(|(_, l)| l.trim().is_empty() || heading(l)).unwrap_or(0);
        let last = lines.len() - 1;
        for (i, (start, l)) in lines.iter().enumerate().skip(from) {
            let nl = norm(l);
            if nl.is_empty() {
                continue;
            }
            // The same line again, or the cut one begun again.
            if nl == nf || (i == last && nf.starts_with(&nl)) {
                return format!("{}{}", &done[..*start], rest.trim_start());
            }
        }
    }
    // The end said again word for word (or a bracket cut after: "[" then "[Outro]").
    for k in (1..=rest.len().min(300)).rev() {
        if rest.is_char_boundary(k) && done.ends_with(&rest[..k]) && (k >= 4 || !rest[..k].chars().any(char::is_alphanumeric)) {
            return format!("{done}{}", &rest[k..]);
        }
    }
    // Cut at a line's end: the next line starts on its own (lyrics, lists).
    let short_lines = lines.len() >= 3 && done.len() / lines.len() < 80;
    if done.ends_with(|c: char| c.is_alphanumeric() || ",.!?;:'\")".contains(c)) && rest.starts_with(|c: char| c.is_uppercase() || c == '[') && short_lines {
        return format!("{done}\n{rest}");
    }
    format!("{done}{rest}")
}

/// A saved call's arguments as JSON the engine can read: a call cut off at
/// the length limit ("{\"text\": \"Lyrics for…") is sent as `{}`, or the
/// engine refused every later reply in that chat.
fn whole_arguments(args: &str) -> String {
    if serde_json::from_str::<Value>(args).is_ok() {
        args.to_string()
    } else {
        "{}".into()
    }
}

/// A call cut off partway: its arguments aren't whole JSON.
fn cut_call(r: &Finished) -> bool {
    r.tool_calls.iter().any(|c| !c.arguments.trim().is_empty() && serde_json::from_str::<Value>(&c.arguments).is_err())
}

/// What's said when a reply stopped at its length limit.
const PAST_LIMIT: &str = "[Your reply above stopped at the length limit, partway through. Go on from exactly where it stopped: no greeting, nothing repeated, just the rest.]";

/// A reply whose answer reached its length limit partway (a song's lyrics
/// stopped after the second verse): carried on where it stopped, up to three
/// more times, without thinking again, so the whole answer arrives.
async fn go_on_past_the_limit<F: FnMut(Delta)>(ep: &Endpoint, messages: &[Value], reply: &mut Finished, cancel: &AtomicBool, on_delta: &mut F) -> Result<(), String> {
    let mut more = true;
    let mut times = 0;
    while more && times < 3 && !reply.content.trim().is_empty() && reply.tool_calls.is_empty() && !reply.cancelled && !cancel.load(Ordering::Relaxed) {
        let mut quick = ep.clone();
        quick.extra["chat_template_kwargs"] = json!({ "enable_thinking": false, "reasoning_effort": "low" });
        quick.extra["reasoning_effort"] = json!("low");
        let mut asked = messages.to_vec();
        asked.push(json!({ "role": "assistant", "content": reply.content }));
        asked.push(json!({ "role": "user", "content": PAST_LIMIT }));
        let (rest, _, again) = stream_local(&quick, asked, None, cancel, on_delta).await?;
        reply.content = join_continued(&reply.content, &rest.content);
        reply.completion_tokens = match (reply.completion_tokens, rest.completion_tokens) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        };
        reply.cancelled = rest.cancelled;
        more = again;
        times += 1;
    }
    if times > 0 {
        crate::applog::line("chat", &format!("A reply reached its length limit and was carried on {times} time(s)"));
    }
    Ok(())
}

/// Two parts of one reply, a blank line between (either may be empty).
pub fn joined(a: &str, b: &str) -> String {
    match (a.trim().is_empty(), b.trim().is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{}\n\n{}", a.trim_end(), b.trim_start()),
    }
}

/// What it thought, for its answer: a loop at the end said once, and the
/// middle of a long think left out (about `ctx / 8` tokens kept).
pub(crate) fn thought_so_far(thinking: &str, ctx: usize) -> String {
    let text = match loop_at_end(thinking) {
        Some((start, unit)) => thinking.get(..start + unit).unwrap_or(thinking),
        None => thinking,
    };
    let keep = (ctx / 2).clamp(1500, 12_000);
    if text.chars().count() > keep {
        cut_middle(text.trim(), keep)
    } else {
        text.trim().to_string()
    }
}

/// One reply from the engine on this PC, and whether it was cut short (at
/// its length limit, or stopped while repeating itself in its thinking).
async fn stream_local<F: FnMut(Delta)>(ep: &Endpoint, messages: Vec<Value>, tools: Option<&Value>, cancel: &AtomicBool, on_delta: &mut F) -> Result<(Finished, bool, bool), String> {
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
        // The unchanged start of the request (instructions, tools, earlier
        // steps) is read from the engine's cache instead of again.
        "cache_prompt": true,
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

    let mut out = Finished::default();
    // Bytes until a whole line is in: a character split between two chunks
    // would otherwise come out garbled.
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    let offered = offered_names(tools);
    let mut cut_short = false;
    // Stopped by its length limit (not a loop): the answer may end mid-way.
    let mut at_length = false;
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
                cut_short = true;
                at_length = true;
            }
            if let Some(delta) = v.pointer("/choices/0/delta") {
                if let Some(t) = delta.get("reasoning_content").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                    out.thinking.push_str(t);
                    on_delta(Delta::Thinking(t.to_string()));
                    // Thinking the same words again and again (a small model
                    // wrote "IEX Cloud, IEX," until its allowance ran out):
                    // stop it there; `stream` has it answer from the rest.
                    if loop_at_end(&out.thinking).is_some() {
                        cut_short = true;
                        break 'read;
                    }
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
                    // Stuck saying the same line (or words) over and over: stop it there.
                    if (t.contains('\n') && repeats_itself(&out.content)) || loop_at_end(&out.content).is_some() {
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
    Ok((out, cut_short, at_length))
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

/// The same words said again and again at the end of `text`, on one line
/// or many (a small model stuck in a loop): where the run starts and how
/// long one of them is. Words only: a row of numbers or dashes is fine.
fn loop_at_end(text: &str) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let n = b.len();
    for unit in 4..=300 {
        // At least 4 times, and 300 bytes in all ("IEX Cloud, IEX, " 19 times).
        let span = (unit * 4).max(300);
        if span > n {
            break;
        }
        if !(n - span..n - unit).all(|i| b[i] == b[i + unit]) {
            continue;
        }
        let Some(said) = text.get(n - unit..) else { continue };
        if said.chars().filter(|c| c.is_alphabetic()).count() < 3 {
            continue;
        }
        let mut start = n - span;
        while start > 0 && b[start - 1] == b[start - 1 + unit] {
            start -= 1;
        }
        while !text.is_char_boundary(start) {
            start += 1;
        }
        return Some((start, unit));
    }
    None
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

/// A reply's thinking (all its steps) in a few short points, shown with the
/// whole of it. None if it gives nothing usable.
pub async fn thought_summary(ep: &Endpoint, thinking: &str) -> Option<String> {
    let messages = vec![
        json!({ "role": "system", "content": "You sum up an assistant's thinking for the person it was helping. Reply with 2 to 5 short bullet points (\"- \"): \
                                              what it worked out, what it found and what it decided. Plain words; no heading, no preamble." }),
        json!({ "role": "user", "content": thought_so_far(thinking, ep.ctx as usize) }),
    ];
    let raw = complete(ep, messages, json!({}), 300).await.ok()?;
    let points: Vec<&str> = raw.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    (!points.is_empty()).then(|| points.join("\n"))
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

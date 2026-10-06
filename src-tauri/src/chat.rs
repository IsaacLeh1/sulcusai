// SPDX-License-Identifier: AGPL-3.0-only
//! Builds the prompt (profile + history within the context window) and
//! streams the model's reply to the window as events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};

use crate::db::{Message, Profile};
use crate::engine::Endpoint;
use crate::net;

/// Tokens each message adds for its role markers in the chat template.
const PER_MESSAGE_OVERHEAD: u32 = 6;

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
    pub history_tokens: u32,
    pub reply_reserve: u32,
    pub dropped_messages: usize,
    /// Prompt + reply tokens of the last finished turn, as the engine counted them.
    pub last_total: Option<u32>,
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

/// Keeps the newest messages that fit; returns them and the context breakdown.
pub async fn fit_history(
    ep: &Endpoint,
    base: &str,
    about: &str,
    history: &[Message],
) -> Result<(Vec<Message>, ContextInfo), String> {
    let system_tokens = count_tokens(ep, base).await? + PER_MESSAGE_OVERHEAD;
    let profile_tokens = count_tokens(ep, about).await?;
    let reply_reserve = (ep.ctx / 4).clamp(256, 2048);
    let budget = ep.ctx.saturating_sub(system_tokens + profile_tokens + reply_reserve);

    let mut kept: Vec<Message> = Vec::new();
    let mut used = 0u32;
    for m in history.iter().rev() {
        let t = count_tokens(ep, &m.content).await? + PER_MESSAGE_OVERHEAD;
        if used + t > budget && !kept.is_empty() {
            break;
        }
        used += t;
        kept.push(m.clone());
    }
    kept.reverse();
    let info = ContextInfo {
        ctx: ep.ctx,
        system_tokens,
        profile_tokens,
        history_tokens: used,
        reply_reserve,
        dropped_messages: history.len() - kept.len(),
        last_total: None,
    };
    Ok((kept, info))
}

pub enum Delta {
    Content(String),
    Thinking(String),
}

pub struct Finished {
    pub content: String,
    pub thinking: String,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub tps: Option<f64>,
    pub cancelled: bool,
}

/// Streams a reply. `on_delta` gets text as it arrives.
pub async fn stream_reply(
    ep: &Endpoint,
    system: &str,
    history: &[Message],
    cancel: &AtomicBool,
    mut on_delta: impl FnMut(Delta),
) -> Result<Finished, String> {
    let mut messages = vec![json!({ "role": "system", "content": system })];
    messages.extend(history.iter().map(|m| json!({ "role": m.role, "content": m.content })));
    let body = json!({
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
        "temperature": 0.7,
    });

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
        return Err(format!("The engine returned {status}: {detail}"));
    }

    let mut out = Finished {
        content: String::new(),
        thinking: String::new(),
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
    Ok(out)
}

/// A short sidebar title from the first message.
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

    #[test]
    fn title_uses_first_nonempty_line_and_truncates() {
        assert_eq!(title_from("\n  hello there \nmore"), "hello there");
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
}

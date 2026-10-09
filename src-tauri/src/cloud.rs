// SPDX-License-Identifier: AGPL-3.0-only
//! The Cloud level: chat models from providers the user turns on with their
//! own API keys. Only used at the Cloud level, keys are encrypted at rest,
//! personal details can be swapped for placeholders on the way out, every
//! request is in the activity log, and spend is tracked against a monthly
//! budget.
//!
//! Anthropic is called through its own Messages API; OpenAI, Google Gemini,
//! OpenRouter and other services through their OpenAI-compatible APIs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures_util::StreamExt;
use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::catalog::{Arch, License, ModelSpec, Ratings, Variant, VisionFile};
use crate::chat::{Delta, Finished};
use crate::crypto::Cipher;
use crate::db::{self, InstalledModel, ToolCall};
use crate::engine::Endpoint;
use crate::net::{self, Connectivity, Purpose};
use crate::{AppState, AppStateRef};

const KEY: &str = "cloud";
const SPEND_KEY: &str = "cloud_spend";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The OpenAI chat completions API (OpenAI, Gemini, OpenRouter, others).
    Openai,
    /// Anthropic's Messages API.
    Anthropic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CloudModel {
    /// The provider's id for it, e.g. "claude-opus-5-5".
    pub id: String,
    pub name: String,
    /// Context window in tokens.
    pub ctx: u32,
    pub max_output: u32,
    pub vision: bool,
    /// US dollars per million tokens, if known.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub base_url: String,
    pub enabled: bool,
    /// Encrypted with the app's key; empty when none is set.
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub models: Vec<CloudModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    pub providers: Vec<Provider>,
    /// Monthly spending limit in US dollars.
    pub budget: Option<f64>,
    /// Replace email addresses, phone numbers and similar details with
    /// placeholders before anything is sent.
    pub redact: bool,
}

impl Default for CloudSettings {
    fn default() -> Self {
        CloudSettings { providers: Vec::new(), budget: None, redact: true }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Spend {
    /// "2026-10".
    pub month: String,
    pub usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub requests: u64,
}

pub fn settings(conn: &Connection) -> CloudSettings {
    db::get(conn, KEY).unwrap_or_default()
}

fn save(conn: &Connection, s: &CloudSettings) -> Result<(), String> {
    db::set(conn, KEY, s)
}

fn this_month() -> String {
    chrono::Local::now().format("%Y-%m").to_string()
}

pub fn spend(conn: &Connection) -> Spend {
    let s: Spend = db::get(conn, SPEND_KEY).unwrap_or_default();
    if s.month == this_month() {
        s
    } else {
        Spend { month: this_month(), ..Default::default() }
    }
}

/// The providers people pick from.
pub fn presets() -> Vec<Provider> {
    let p = |id: &str, name: &str, kind, url: &str| Provider { id: id.into(), name: name.into(), kind, base_url: url.into(), enabled: false, key: String::new(), models: Vec::new() };
    vec![
        p("anthropic", "Anthropic (Claude)", Kind::Anthropic, "https://api.anthropic.com"),
        p("openai", "OpenAI", Kind::Openai, "https://api.openai.com/v1"),
        p("gemini", "Google Gemini", Kind::Openai, "https://generativelanguage.googleapis.com/v1beta/openai"),
        p("openrouter", "OpenRouter", Kind::Openai, "https://openrouter.ai/api/v1"),
    ]
}

/// Anthropic's list prices ($ per million tokens, input/output), as of
/// October 2026. People can change them in Settings.
fn anthropic_price(id: &str) -> Option<(f64, f64)> {
    Some(match id {
        "claude-fable-5-1" | "claude-fable-5" | "claude-mythos-5-1" => (10.0, 50.0),
        "claude-opus-5-5" => (4.0, 20.0),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => (5.0, 25.0),
        "claude-sonnet-5-5" | "claude-sonnet-5" => (2.0, 10.0),
        "claude-sonnet-4-6" => (3.0, 15.0),
        "claude-haiku-5-5" => (0.1, 0.5),
        "claude-haiku-4-5" => (1.0, 5.0),
        _ => return None,
    })
}

/// Claude models that take adaptive thinking and an effort level.
fn adaptive(id: &str) -> bool {
    ["claude-fable-5", "claude-mythos-5", "claude-opus-5", "claude-sonnet-5", "claude-haiku-5-5", "claude-opus-4-8", "claude-opus-4-7", "claude-opus-4-6", "claude-sonnet-4-6"]
        .iter()
        .any(|p| id.starts_with(p))
}

/// Claude models that should fall back to another model on a refusal.
fn wants_fallback(id: &str) -> bool {
    matches!(id, "claude-fable-5-1" | "claude-opus-5-5" | "claude-opus-5" | "claude-sonnet-5-5")
}

// ---------- ids ----------

/// "cloud:<provider>:<model>" for a cloud model in the app's model lists.
pub fn model_ref(provider: &str, model: &str) -> String {
    format!("cloud:{provider}:{model}")
}

pub fn parse_ref(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix("cloud:")?.split_once(':')
}

pub fn is_cloud(id: &str) -> bool {
    id.starts_with("cloud:")
}

// ---------- the endpoint ----------

/// Where a chat goes when it uses a cloud model.
pub struct Target {
    pub kind: Kind,
    pub provider: String,
    pub base_url: String,
    key: String,
    pub model: CloudModel,
    pub redact: bool,
    state: Arc<AppState>,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").field("provider", &self.provider).field("model", &self.model.id).finish_non_exhaustive()
    }
}

/// The endpoint, model card and stand-in install record for a cloud model.
pub fn endpoint(state: &Arc<AppState>, id: &str, cipher: &Cipher) -> Result<(Endpoint, ModelSpec, InstalledModel), String> {
    let (pid, mid) = parse_ref(id).ok_or("Unknown cloud model.")?;
    let (level, s, spent) = {
        let conn = state.db.lock().unwrap();
        (db::settings(&conn).connectivity, settings(&conn), spend(&conn))
    };
    if level != Connectivity::Cloud {
        return Err("This chat uses a cloud model, which needs the Cloud level. Switch to it in the status bar, or pick a model on this PC.".into());
    }
    let p = s.providers.iter().find(|p| p.id == pid).ok_or("That cloud provider was removed. Pick another model.")?;
    if !p.enabled {
        return Err(format!("{} is turned off in Settings → Cloud models.", p.name));
    }
    let m = p.models.iter().find(|m| m.id == mid).ok_or("That cloud model was removed. Pick another model.")?;
    if let Some(b) = s.budget {
        if spent.usd >= b {
            return Err(format!("This month's cloud budget (${b:.2}) is used up. Raise it in Settings → Cloud models, or pick a model on this PC."));
        }
    }
    let key = if p.key.is_empty() { String::new() } else { cipher.decrypt(&p.key)? };
    if key.is_empty() {
        return Err(format!("Add your {} API key in Settings → Cloud models.", p.name));
    }
    let ctx = m.ctx.max(4096);
    let target = Target { kind: p.kind, provider: p.name.clone(), base_url: p.base_url.trim_end_matches('/').to_string(), key, model: m.clone(), redact: s.redact, state: state.clone() };
    let ep = Endpoint { port: 0, key: String::new(), ctx, extra: crate::advanced::get(state).request_extra(), cloud: Some(Arc::new(target)) };
    let spec = ModelSpec {
        id: id.to_string(),
        name: m.name.clone(),
        publisher: p.name.clone(),
        source: p.base_url.clone(),
        description: format!("Runs on {}'s servers. What you send goes to them.", p.name),
        license: License { name: format!("{}'s terms", p.name), url: String::new(), commercial: true, note: None },
        tags: vec!["cloud".into()],
        params_b: 0.0,
        arch: Arch { n_layer: 1, n_kv_heads: 1, head_dim: 1, max_ctx: ctx, active_fraction: 1.0 },
        default_ctx: ctx,
        tools: true,
        variants: vec![Variant { quant: "cloud".into(), quality: "Cloud".into(), file: String::new(), url: String::new(), size: 0, sha256: String::new() }],
        ratings: Ratings::default(),
        vision: m.vision.then(|| VisionFile { file: String::new(), url: String::new(), size: 0, sha256: String::new() }),
        local: None,
    };
    let installed = InstalledModel { model_id: id.to_string(), quant: "cloud".into(), path: String::new(), size: 0, installed_at: 0, tps: None };
    Ok((ep, spec, installed))
}

/// Tokens in text, estimated (cloud providers have no local counter).
pub fn estimate_tokens(text: &str) -> u32 {
    (text.chars().count() as f64 / 3.5).ceil() as u32
}

// ---------- personal details ----------

static PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        ("email", Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b").unwrap()),
        ("card number", Regex::new(r"\b(?:\d[ -]?){12,18}\d\b").unwrap()),
        ("SSN", Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap()),
        ("phone", Regex::new(r"(?:\+\d{1,3}[ .-]?)?\(?\b\d{3}\)?[ .-]?\d{3}[ .-]?\d{4}\b").unwrap()),
    ]
});

fn luhn(digits: &str) -> bool {
    let d: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&d.len()) {
        return false;
    }
    let sum: u32 = d.iter().rev().enumerate().map(|(i, &n)| if i % 2 == 1 { let x = n * 2; if x > 9 { x - 9 } else { x } } else { n }).sum();
    sum % 10 == 0
}

/// Swaps personal details for placeholders and puts them back in replies.
#[derive(Default)]
pub struct Redactor {
    /// Placeholder → original.
    map: HashMap<String, String>,
    count: HashMap<&'static str, usize>,
}

impl Redactor {
    pub fn redact(&mut self, text: &str) -> String {
        let mut out = text.to_string();
        for (label, re) in PATTERNS.iter() {
            let mut next = String::with_capacity(out.len());
            let mut last = 0;
            for m in re.find_iter(&out) {
                if *label == "card number" && !luhn(m.as_str()) {
                    continue;
                }
                let original = m.as_str().to_string();
                let placeholder = match self.map.iter().find(|(_, v)| **v == original) {
                    Some((k, _)) => k.clone(),
                    None => {
                        let n = self.count.entry(label).or_default();
                        *n += 1;
                        let p = format!("[{label} {n}]");
                        self.map.insert(p.clone(), original);
                        p
                    }
                };
                next.push_str(&out[last..m.start()]);
                next.push_str(&placeholder);
                last = m.end();
            }
            next.push_str(&out[last..]);
            out = next;
        }
        out
    }

    pub fn restore(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (k, v) in &self.map {
            out = out.replace(k, v);
        }
        out
    }

    pub fn used(&self) -> usize {
        self.map.len()
    }
}

/// Redacts the text of every message (system, user, tool) in place.
fn redact_messages(messages: &mut [Value], r: &mut Redactor) {
    for m in messages.iter_mut() {
        if m["role"] == "assistant" {
            continue;
        }
        match &mut m["content"] {
            Value::String(s) => *s = r.redact(s),
            Value::Array(parts) => {
                for p in parts {
                    if let Some(Value::String(t)) = p.get_mut("text") {
                        *t = r.redact(t);
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------- requests ----------

fn client(state: &AppState) -> Result<reqwest::Client, String> {
    net::external_client(state.settings().connectivity, Purpose::Cloud, false)
}

/// OpenAI chat messages → Anthropic's system text, messages and tools.
fn to_anthropic(messages: &[Value], tools: Option<&Value>) -> (String, Vec<Value>, Vec<Value>) {
    let mut system = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    // Thinking blocks are replayed only for the turn in progress (a run of
    // tool calls); earlier turns keep their text and tool calls.
    let last_user = messages.iter().rposition(|m| m["role"] == "user").unwrap_or(0);
    let push = |out: &mut Vec<Value>, role: &str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(prev) if prev["role"] == role => prev["content"].as_array_mut().unwrap().extend(blocks),
            _ => out.push(json!({ "role": role, "content": blocks })),
        }
    };
    for (i, m) in messages.iter().enumerate() {
        match m["role"].as_str().unwrap_or("") {
            "system" => system.push(m["content"].as_str().unwrap_or("").to_string()),
            "user" => {
                let blocks = match &m["content"] {
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| match p["type"].as_str() {
                            Some("text") => Some(json!({ "type": "text", "text": p["text"] })),
                            Some("image_url") => {
                                let url = p.pointer("/image_url/url")?.as_str()?;
                                let (head, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
                                Some(json!({ "type": "image", "source": { "type": "base64", "media_type": head, "data": data } }))
                            }
                            _ => None,
                        })
                        .collect(),
                    v => vec![json!({ "type": "text", "text": v.as_str().unwrap_or("") })],
                };
                push(&mut out, "user", blocks);
            }
            "assistant" => {
                let blocks: Vec<Value> = match m.get("cloud_blocks").and_then(Value::as_array) {
                    Some(raw) => raw.iter().filter(|b| i > last_user || !matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking"))).cloned().collect(),
                    None => {
                        let mut b = Vec::new();
                        if let Some(t) = m["content"].as_str().filter(|t| !t.trim().is_empty()) {
                            b.push(json!({ "type": "text", "text": t }));
                        }
                        for c in m["tool_calls"].as_array().into_iter().flatten() {
                            let args = c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
                            let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                            b.push(json!({ "type": "tool_use", "id": c["id"], "name": c.pointer("/function/name"), "input": input }));
                        }
                        b
                    }
                };
                push(&mut out, "assistant", blocks);
            }
            "tool" => push(&mut out, "user", vec![json!({ "type": "tool_result", "tool_use_id": m["tool_call_id"], "content": m["content"].as_str().unwrap_or("") })]),
            _ => {}
        }
    }
    if out.first().is_some_and(|m| m["role"] != "user") {
        out.insert(0, json!({ "role": "user", "content": [{ "type": "text", "text": "(The conversation continues.)" }] }));
    }
    let tools = tools
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| json!({ "name": t.pointer("/function/name"), "description": t.pointer("/function/description"), "input_schema": t.pointer("/function/parameters") }))
        .collect();
    (system.join("\n\n"), out, tools)
}

/// Sends one chat to the provider and streams the reply back.
pub async fn stream(t: &Target, extra: &Value, mut messages: Vec<Value>, tools: Option<&Value>, cancel: &AtomicBool, mut on_delta: impl FnMut(Delta)) -> Result<Finished, String> {
    let mut redactor = Redactor::default();
    if t.redact {
        redact_messages(&mut messages, &mut redactor);
    }
    let client = client(&t.state)?;
    let started = std::time::Instant::now();
    let mut restore = |d: Delta| {
        on_delta(match d {
            Delta::Content(s) => Delta::Content(redactor.restore(&s)),
            Delta::Thinking(s) => Delta::Thinking(redactor.restore(&s)),
        })
    };
    let mut out = match t.kind {
        Kind::Anthropic => anthropic_stream(&client, t, extra, &messages, tools, cancel, &mut restore).await?,
        Kind::Openai => openai_stream(&client, t, extra, &messages, tools, cancel, &mut restore).await?,
    };
    crate::chat::rescue_tool_calls(&mut out, tools);
    out.content = redactor.restore(&out.content);
    out.thinking = redactor.restore(&out.thinking);
    for c in &mut out.tool_calls {
        c.arguments = redactor.restore(&c.arguments);
        if c.id.is_empty() {
            c.id = format!("call_{}", uuid::Uuid::new_v4().simple());
        }
    }
    out.tool_calls.retain(|c| !c.name.is_empty());
    if let (Some(n), true) = (out.completion_tokens, started.elapsed().as_secs_f64() > 0.5) {
        out.tps = Some((n as f64 / started.elapsed().as_secs_f64() * 10.0).round() / 10.0);
    }
    record(t, out.prompt_tokens.unwrap_or(0), out.completion_tokens.unwrap_or(0), redactor.used());
    Ok(out)
}

/// One reply, not streamed (for titles and other short work).
pub async fn complete(t: &Target, messages: Vec<Value>) -> Result<String, String> {
    let done = stream(t, &json!({}), messages, None, &AtomicBool::new(false), |_| {}).await?;
    Ok(done.content)
}

fn record(t: &Target, input: u32, output: u32, redacted: usize) {
    let cost = input as f64 * t.model.price_in.unwrap_or(0.0) / 1e6 + output as f64 * t.model.price_out.unwrap_or(0.0) / 1e6;
    {
        let conn = t.state.db.lock().unwrap();
        let mut s = spend(&conn);
        s.usd += cost;
        s.input_tokens += input as u64;
        s.output_tokens += output as u64;
        s.requests += 1;
        let _ = db::set(&conn, SPEND_KEY, &s);
    }
    let priced = if t.model.price_in.is_some() { format!(", about ${cost:.4}") } else { String::new() };
    let hidden = if redacted > 0 { format!("; {redacted} personal detail(s) replaced with placeholders") } else { String::new() };
    t.state.log("network", &format!("Sent a chat to {} ({}): {input} tokens in, {output} out{priced}{hidden}", t.provider, t.model.name));
}

async fn error_text(resp: reqwest::Response, who: &str) -> String {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or(text);
    match status.as_u16() {
        401 | 403 => format!("{who} didn't accept the API key ({status}). Check it in Settings → Cloud models."),
        429 => format!("{who} says you've hit a rate or spending limit: {detail}"),
        _ => format!("{who} returned {status}: {detail}"),
    }
}

/// Reads server-sent events, calling `on` with each data payload.
async fn sse(resp: reqwest::Response, cancel: &AtomicBool, mut on: impl FnMut(&Value) -> bool) -> Result<bool, String> {
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Ok(true);
        }
        let chunk = chunk.map_err(|e| format!("The reply was interrupted: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line: String = buf.drain(..=nl).collect();
            let Some(data) = line.trim().strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(false);
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if !on(&v) {
                    return Ok(false);
                }
            }
        }
    }
    Ok(false)
}

fn empty() -> Finished {
    Finished { content: String::new(), thinking: String::new(), tool_calls: Vec::new(), prompt_tokens: None, completion_tokens: None, tps: None, cancelled: false, raw: None }
}

async fn anthropic_stream(client: &reqwest::Client, t: &Target, extra: &Value, messages: &[Value], tools: Option<&Value>, cancel: &AtomicBool, on_delta: &mut impl FnMut(Delta)) -> Result<Finished, String> {
    let (system, msgs, tools) = to_anthropic(messages, tools);
    let id = t.model.id.as_str();
    let mut body = json!({
        "model": id,
        "max_tokens": t.model.max_output.clamp(1024, 64_000),
        "messages": msgs,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    let mut req = client.post(format!("{}/v1/messages", t.base_url)).header("x-api-key", &t.key).header("anthropic-version", ANTHROPIC_VERSION);
    if adaptive(id) {
        // Readable summaries of its reasoning for the thinking panel.
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
        // Advanced mode's thinking switch maps to effort (current Claude
        // models always think; temperature and top-p aren't accepted).
        match extra.pointer("/chat_template_kwargs/enable_thinking").and_then(Value::as_bool) {
            Some(false) => body["output_config"] = json!({ "effort": "low" }),
            Some(true) => body["output_config"] = json!({ "effort": "high" }),
            None => {}
        }
    }
    if wants_fallback(id) {
        body["fallbacks"] = json!("default");
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = req.json(&body).timeout(Duration::from_secs(900)).send().await.map_err(|e| format!("Couldn't reach {}: {e}", t.provider))?;
    if !resp.status().is_success() {
        return Err(error_text(resp, &t.provider).await);
    }
    let mut out = empty();
    // Blocks by index, to save for replay (thinking keeps its signature).
    let mut blocks: Vec<Value> = Vec::new();
    let mut json_parts: HashMap<usize, String> = HashMap::new();
    let mut stop = String::new();
    let mut refusal = None;
    out.cancelled = sse(resp, cancel, |v| {
        match v["type"].as_str() {
            Some("message_start") => {
                let u = &v["message"]["usage"];
                let input = u["input_tokens"].as_u64().unwrap_or(0) + u["cache_creation_input_tokens"].as_u64().unwrap_or(0) + u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                out.prompt_tokens = Some(input as u32);
            }
            Some("content_block_start") => {
                let i = v["index"].as_u64().unwrap_or(0) as usize;
                while blocks.len() <= i {
                    blocks.push(Value::Null);
                }
                let b = &v["content_block"];
                blocks[i] = match b["type"].as_str() {
                    Some("text") => json!({ "type": "text", "text": "" }),
                    Some("thinking") => json!({ "type": "thinking", "thinking": "", "signature": "" }),
                    Some("redacted_thinking") => b.clone(),
                    Some("tool_use") => json!({ "type": "tool_use", "id": b["id"], "name": b["name"], "input": {} }),
                    _ => Value::Null,
                };
            }
            Some("content_block_delta") => {
                let i = v["index"].as_u64().unwrap_or(0) as usize;
                let d = &v["delta"];
                let Some(b) = blocks.get_mut(i) else { return true };
                match d["type"].as_str() {
                    Some("text_delta") => {
                        let s = d["text"].as_str().unwrap_or("");
                        if let Some(Value::String(t)) = b.get_mut("text") {
                            t.push_str(s);
                        }
                        on_delta(Delta::Content(s.to_string()));
                    }
                    Some("thinking_delta") => {
                        let s = d["thinking"].as_str().unwrap_or("");
                        if let Some(Value::String(t)) = b.get_mut("thinking") {
                            t.push_str(s);
                        }
                        on_delta(Delta::Thinking(s.to_string()));
                    }
                    Some("signature_delta") => {
                        if let Some(Value::String(t)) = b.get_mut("signature") {
                            t.push_str(d["signature"].as_str().unwrap_or(""));
                        }
                    }
                    Some("input_json_delta") => json_parts.entry(i).or_default().push_str(d["partial_json"].as_str().unwrap_or("")),
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(n) = v.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    out.completion_tokens = Some(n as u32);
                }
                if let Some(s) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    stop = s.to_string();
                }
                if let Some(c) = v.pointer("/delta/stop_details/category").and_then(Value::as_str) {
                    refusal = Some(c.to_string());
                }
            }
            Some("error") => {
                refusal = v.pointer("/error/message").and_then(Value::as_str).map(|m| format!("error: {m}"));
                return false;
            }
            _ => {}
        }
        true
    })
    .await?;
    for (i, raw) in json_parts {
        if let Some(b) = blocks.get_mut(i) {
            b["input"] = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
        }
    }
    blocks.retain(|b| !b.is_null());
    for b in &blocks {
        match b["type"].as_str() {
            Some("text") => out.content.push_str(b["text"].as_str().unwrap_or("")),
            Some("thinking") => out.thinking.push_str(b["thinking"].as_str().unwrap_or("")),
            Some("tool_use") => out.tool_calls.push(ToolCall { id: b["id"].as_str().unwrap_or("").into(), name: b["name"].as_str().unwrap_or("").into(), arguments: b["input"].to_string() }),
            _ => {}
        }
    }
    if let Some(m) = refusal.as_ref().and_then(|r| r.strip_prefix("error: ")) {
        return Err(format!("{} stopped with an error: {m}", t.provider));
    }
    if stop == "refusal" && out.content.trim().is_empty() && out.tool_calls.is_empty() {
        let why = refusal.map(|c| format!(" ({c})")).unwrap_or_default();
        return Err(format!("{} declined to answer this{why}. Try rephrasing, or use a model on this PC.", t.provider));
    }
    out.raw = Some(Value::Array(blocks));
    Ok(out)
}

async fn openai_stream(client: &reqwest::Client, t: &Target, extra: &Value, messages: &[Value], tools: Option<&Value>, cancel: &AtomicBool, on_delta: &mut impl FnMut(Delta)) -> Result<Finished, String> {
    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if let Some(o) = m.as_object_mut() {
                o.remove("cloud_blocks");
            }
            m
        })
        .collect();
    let mut body = json!({ "model": t.model.id, "messages": msgs, "stream": true, "stream_options": { "include_usage": true } });
    if let Some(tl) = tools.filter(|t| t.as_array().is_some_and(|a| !a.is_empty())) {
        body["tools"] = tl.clone();
    }
    // Only the standard sampling settings (local-engine ones are left out).
    for k in ["temperature", "top_p", "presence_penalty", "frequency_penalty", "seed"] {
        if let Some(v) = extra.get(k) {
            body[k] = v.clone();
        }
    }
    let resp = client
        .post(format!("{}/chat/completions", t.base_url))
        .bearer_auth(&t.key)
        .header("HTTP-Referer", "https://github.com/IsaacLeh1/sulcusai")
        .header("X-Title", "SulcusAI")
        .json(&body)
        .timeout(Duration::from_secs(900))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach {}: {e}", t.provider))?;
    if !resp.status().is_success() {
        return Err(error_text(resp, &t.provider).await);
    }
    let mut out = empty();
    out.cancelled = sse(resp, cancel, |v| {
        if let Some(d) = v.pointer("/choices/0/delta") {
            for key in ["reasoning_content", "reasoning"] {
                if let Some(s) = d.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    out.thinking.push_str(s);
                    on_delta(Delta::Thinking(s.to_string()));
                }
            }
            if let Some(s) = d.get("content").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                out.content.push_str(s);
                on_delta(Delta::Content(s.to_string()));
            }
            for tc in d.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                crate::chat::merge_tool_delta(&mut out.tool_calls, tc);
            }
        }
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            out.prompt_tokens = u["prompt_tokens"].as_u64().map(|n| n as u32);
            out.completion_tokens = u["completion_tokens"].as_u64().map(|n| n as u32);
        }
        true
    })
    .await?;
    Ok(out)
}

// ---------- listing a provider's models ----------

async fn list_models(p: &Provider, key: &str, client: &reqwest::Client) -> Result<Vec<CloudModel>, String> {
    let base = p.base_url.trim_end_matches('/');
    let mut out = Vec::new();
    match p.kind {
        Kind::Anthropic => {
            let resp = client.get(format!("{base}/v1/models?limit=100")).header("x-api-key", key).header("anthropic-version", ANTHROPIC_VERSION).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(error_text(resp, &p.name).await);
            }
            let v: Value = resp.json().await.map_err(|e| e.to_string())?;
            for m in v["data"].as_array().into_iter().flatten() {
                let id = m["id"].as_str().unwrap_or_default().to_string();
                let price = anthropic_price(&id);
                out.push(CloudModel {
                    name: m["display_name"].as_str().unwrap_or(&id).to_string(),
                    ctx: m["max_input_tokens"].as_u64().unwrap_or(200_000) as u32,
                    max_output: m["max_tokens"].as_u64().unwrap_or(16_000) as u32,
                    vision: m.pointer("/capabilities/image_input/supported").and_then(Value::as_bool).unwrap_or(true),
                    price_in: price.map(|p| p.0),
                    price_out: price.map(|p| p.1),
                    id,
                });
            }
        }
        Kind::Openai => {
            let resp = client.get(format!("{base}/models")).bearer_auth(key).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(error_text(resp, &p.name).await);
            }
            let v: Value = resp.json().await.map_err(|e| e.to_string())?;
            for m in v["data"].as_array().into_iter().flatten() {
                let id = m["id"].as_str().unwrap_or_default().trim_start_matches("models/").to_string();
                // OpenRouter adds names, context sizes and prices (per token).
                let per_m = |k: &str| m.pointer(&format!("/pricing/{k}")).and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or(v.as_f64())).map(|p| (p * 1e6 * 1000.0).round() / 1000.0);
                let modalities = m.pointer("/architecture/input_modalities").and_then(Value::as_array);
                out.push(CloudModel {
                    name: m["name"].as_str().unwrap_or(&id).to_string(),
                    ctx: m["context_length"].as_u64().unwrap_or(128_000) as u32,
                    max_output: m.pointer("/top_provider/max_completion_tokens").and_then(Value::as_u64).unwrap_or(16_000) as u32,
                    vision: modalities.is_none_or(|a| a.iter().any(|x| x == "image")),
                    price_in: per_m("prompt"),
                    price_out: per_m("completion"),
                    id,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct ProviderView {
    id: String,
    name: String,
    kind: Kind,
    base_url: String,
    enabled: bool,
    has_key: bool,
    models: Vec<CloudModel>,
}

#[derive(Serialize)]
pub struct CloudView {
    providers: Vec<ProviderView>,
    presets: Vec<ProviderView>,
    budget: Option<f64>,
    redact: bool,
    spend: Spend,
}

fn pview(p: &Provider) -> ProviderView {
    ProviderView { id: p.id.clone(), name: p.name.clone(), kind: p.kind, base_url: p.base_url.clone(), enabled: p.enabled, has_key: !p.key.is_empty(), models: p.models.clone() }
}

#[tauri::command]
pub fn cloud_view(state: AppStateRef) -> CloudView {
    let conn = state.db.lock().unwrap();
    let s = settings(&conn);
    let presets = presets().iter().filter(|p| !s.providers.iter().any(|q| q.id == p.id)).map(pview).collect();
    CloudView { providers: s.providers.iter().map(pview).collect(), presets, budget: s.budget, redact: s.redact, spend: spend(&conn) }
}

/// Adds a preset provider, or a custom OpenAI-compatible one.
#[tauri::command]
pub fn add_cloud_provider(state: AppStateRef, preset: Option<String>, name: Option<String>, base_url: Option<String>) -> Result<CloudView, String> {
    state.cipher()?;
    let p = match preset {
        Some(id) => presets().into_iter().find(|p| p.id == id).ok_or("Unknown provider")?,
        None => {
            let url = base_url.unwrap_or_default().trim().trim_end_matches('/').to_string();
            if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1") && !url.starts_with("http://localhost") {
                return Err("Use an https:// address (or a local one).".into());
            }
            let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Custom provider".into());
            Provider { id: format!("custom-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]), name, kind: Kind::Openai, base_url: url, enabled: false, key: String::new(), models: Vec::new() }
        }
    };
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        if !s.providers.iter().any(|q| q.id == p.id) {
            s.providers.push(p.clone());
        }
        save(&conn, &s)?;
    }
    state.log("settings", &format!("Added the cloud provider {}", p.name));
    Ok(cloud_view(state))
}

#[tauri::command]
pub fn remove_cloud_provider(state: AppStateRef, id: String) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.providers.retain(|p| p.id != id);
        save(&conn, &s)?;
    }
    state.log("settings", "Removed a cloud provider and its key");
    Ok(cloud_view(state))
}

/// Sets the key (empty clears it) and on/off for a provider.
#[tauri::command]
pub fn set_cloud_provider(state: AppStateRef, id: String, enabled: bool, key: Option<String>) -> Result<CloudView, String> {
    let c = state.cipher()?;
    let name = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        if let Some(k) = key {
            p.key = if k.trim().is_empty() { String::new() } else { c.encrypt(k.trim()) };
        }
        p.enabled = enabled && !p.key.is_empty();
        let name = p.name.clone();
        save(&conn, &s)?;
        name
    };
    state.log("settings", &format!("Updated the cloud provider {name}"));
    Ok(cloud_view(state))
}

/// The models a provider offers (asks the provider).
#[tauri::command]
pub async fn cloud_models_available(state: AppStateRef<'_>, id: String) -> Result<Vec<CloudModel>, String> {
    let c = state.cipher()?;
    let p = settings(&state.db.lock().unwrap()).providers.into_iter().find(|p| p.id == id).ok_or("Unknown provider")?;
    if p.key.is_empty() {
        return Err(format!("Add your {} API key first.", p.name));
    }
    let key = c.decrypt(&p.key)?;
    let client = client(&state)?;
    state.log("network", &format!("Asked {} which models it offers", p.name));
    list_models(&p, &key, &client).await
}

/// Which of the provider's models appear in the model lists.
#[tauri::command]
pub fn set_cloud_models(state: AppStateRef, id: String, models: Vec<CloudModel>) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        p.models = models;
        save(&conn, &s)?;
    }
    Ok(cloud_view(state))
}

#[tauri::command]
pub fn set_cloud_options(state: AppStateRef, budget: Option<f64>, redact: bool) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.budget = budget.filter(|b| *b > 0.0);
        s.redact = redact;
        save(&conn, &s)?;
    }
    state.log("settings", "Changed the cloud budget or privacy options");
    Ok(cloud_view(state))
}

/// The cloud models people can pick for a chat (enabled providers only).
#[derive(Serialize)]
pub struct CloudChoice {
    pub id: String,
    name: String,
    provider: String,
    vision: bool,
}

#[tauri::command]
pub fn cloud_choices(state: AppStateRef) -> Vec<CloudChoice> {
    choices(&state.db.lock().unwrap())
}

pub fn choices(conn: &Connection) -> Vec<CloudChoice> {
    settings(conn)
        .providers
        .iter()
        .filter(|p| p.enabled && !p.key.is_empty())
        .flat_map(|p| p.models.iter().map(move |m| CloudChoice { id: model_ref(&p.id, &m.id), name: m.name.clone(), provider: p.name.clone(), vision: m.vision }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_and_restores_personal_details() {
        let mut r = Redactor::default();
        let out = r.redact("Email jo@example.com or call (801) 555-0123. Card 4111 1111 1111 1111, SSN 123-45-6789. Order 12345678901234 isn't a card.");
        assert!(!out.contains("jo@example.com") && !out.contains("555-0123") && !out.contains("4111") && !out.contains("123-45-6789"), "{out}");
        assert!(out.contains("[email 1]") && out.contains("[card number 1]") && out.contains("[SSN 1]"), "{out}");
        assert!(out.contains("12345678901234"), "fails the card checksum, so kept: {out}");
        // The same detail gets the same placeholder.
        assert_eq!(r.redact("again jo@example.com"), "again [email 1]");
        assert_eq!(r.restore("I'll write to [email 1]."), "I'll write to jo@example.com.");
    }

    #[test]
    fn converts_a_tool_conversation_for_anthropic() {
        let messages = vec![
            json!({ "role": "system", "content": "Be brief." }),
            json!({ "role": "user", "content": "Weather in Paris?" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "t1", "type": "function", "function": { "name": "weather", "arguments": "{\"place\":\"Paris\"}" } }] }),
            json!({ "role": "tool", "tool_call_id": "t1", "content": "18°C" }),
            json!({ "role": "user", "content": [{ "type": "text", "text": "And this?" }, { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,AAAA" } }] }),
            json!({ "role": "assistant", "content": "x", "cloud_blocks": [{ "type": "thinking", "thinking": "hm", "signature": "s" }, { "type": "tool_use", "id": "t2", "name": "look", "input": {} }] }),
            json!({ "role": "tool", "tool_call_id": "t2", "content": "a cat" }),
        ];
        let tools = json!([{ "type": "function", "function": { "name": "weather", "description": "d", "parameters": { "type": "object" } } }]);
        let (system, msgs, tools) = to_anthropic(&messages, Some(&tools));
        assert_eq!(system, "Be brief.");
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[1]["content"][0]["input"]["place"], "Paris");
        // The tool result and the next user message share one user turn.
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][2]["source"]["media_type"], "image/jpeg");
        // The turn in progress keeps its thinking block, signature and all.
        assert_eq!(msgs[3]["content"][0]["signature"], "s");
        assert_eq!(msgs[4]["content"][0]["tool_use_id"], "t2");
        assert_eq!(tools[0]["input_schema"]["type"], "object");
    }

    #[test]
    fn thinking_from_earlier_turns_is_left_out() {
        let messages = vec![
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": "hello", "cloud_blocks": [{ "type": "thinking", "thinking": "hm", "signature": "s" }, { "type": "text", "text": "hello" }] }),
            json!({ "role": "user", "content": "bye" }),
        ];
        let (_, msgs, _) = to_anthropic(&messages, None);
        assert_eq!(msgs[1]["content"].as_array().unwrap().len(), 1);
        assert_eq!(msgs[1]["content"][0]["type"], "text");
    }

    #[test]
    fn model_refs_round_trip() {
        let id = model_ref("openrouter", "meta-llama/llama-4:free");
        assert_eq!(parse_ref(&id), Some(("openrouter", "meta-llama/llama-4:free")));
        assert!(is_cloud(&id) && !is_cloud("qwen3-1.7b"));
    }
}

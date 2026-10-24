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

/// What a cloud media model makes.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    #[default]
    Image,
    Video,
    Speech,
}

/// A picture, video or voice model the user turned on.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CloudMedia {
    pub id: String,
    pub name: String,
    pub kind: MediaKind,
    /// US dollars per picture, per second of video, or per million
    /// characters read aloud, if known.
    pub price: Option<f64>,
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
    #[serde(default)]
    pub media: Vec<CloudMedia>,
    /// Sign-in cookies from the built-in browser (encrypted; empty when
    /// none), sent with its API calls: a server behind a sign-in page.
    #[serde(default)]
    pub cookies: String,
    /// Its chats go to this user's other PCs that use the same key (sync).
    /// Off: they stay on the PC they were made on.
    #[serde(default)]
    pub share_pcs: bool,
    /// People (linked in People) its chats are shared with, live.
    #[serde(default)]
    pub share_people: Vec<String>,
    /// A proxy only its requests go through (encrypted; empty: none), such
    /// as a VPN's local SOCKS proxy for a server inside a company network.
    #[serde(default)]
    pub proxy: String,
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
    let mut s: CloudSettings = db::get(conn, KEY).unwrap_or_default();
    // An address pasted with its endpoint on the end ("…/v1/models").
    for p in &mut s.providers {
        p.base_url = clean_base(&p.base_url);
    }
    s
}

/// A server's API address as typed: without a trailing slash, or an
/// endpoint pasted on the end ("https://x/v1/models" → "https://x/v1").
pub fn clean_base(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    // Also with the "/v1" the LiteLLM choice adds after it ("…/v1/models/v1").
    let (core, v1) = match u.strip_suffix("/v1") {
        Some(c) if ["/chat/completions", "/completions", "/models", "/messages"].iter().any(|e| c.ends_with(e)) => (c, true),
        _ => (u, false),
    };
    for end in ["/chat/completions", "/completions", "/models", "/messages"] {
        if let Some(cut) = core.strip_suffix(end) {
            let cut = cut.trim_end_matches('/');
            return if v1 && !cut.ends_with("/v1") { format!("{cut}/v1") } else { cut.to_string() };
        }
    }
    u.to_string()
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
    let p = |id: &str, name: &str, kind, url: &str| Provider { id: id.into(), name: name.into(), kind, base_url: url.into(), enabled: false, key: String::new(), models: Vec::new(), media: Vec::new(), cookies: String::new(), share_pcs: false, share_people: Vec::new(), proxy: String::new() };
    vec![
        p("anthropic", "Anthropic (Claude)", Kind::Anthropic, "https://api.anthropic.com"),
        p("openai", "OpenAI", Kind::Openai, "https://api.openai.com/v1"),
        p("gemini", "Google Gemini", Kind::Openai, "https://generativelanguage.googleapis.com/v1beta/openai"),
        p("openrouter", "OpenRouter", Kind::Openai, "https://openrouter.ai/api/v1"),
    ]
}

/// Picture, video and voice models each provider offers through the
/// OpenAI-style media APIs, with list prices as last checked (estimates;
/// people can change them).
pub fn media_suggestions(provider: &str) -> Vec<CloudMedia> {
    let m = |id: &str, name: &str, kind, price: f64| CloudMedia { id: id.into(), name: name.into(), kind, price: Some(price) };
    match provider {
        "openai" => vec![
            m("gpt-image-1", "GPT Image", MediaKind::Image, 0.042),
            m("gpt-image-1-mini", "GPT Image mini", MediaKind::Image, 0.011),
            m("sora-2", "Sora 2", MediaKind::Video, 0.10),
            m("sora-2-pro", "Sora 2 Pro", MediaKind::Video, 0.30),
            m("gpt-4o-mini-tts", "GPT-4o mini voices", MediaKind::Speech, 12.0),
            m("tts-1", "OpenAI TTS", MediaKind::Speech, 15.0),
        ],
        "gemini" => vec![m("imagen-4.0-generate-001", "Imagen 4", MediaKind::Image, 0.04), m("imagen-4.0-fast-generate-001", "Imagen 4 Fast", MediaKind::Image, 0.02)],
        _ => Vec::new(),
    }
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
    /// Sign-in cookies for a server behind a sign-in page (empty: none).
    cookies: String,
    pub model: CloudModel,
    pub redact: bool,
    state: Arc<AppState>,
    /// The chat its replies are for, so what they cost is counted there.
    chat: std::sync::Mutex<Option<String>>,
    /// Its provider's own proxy (empty: none).
    proxy: String,
}

impl Target {
    pub fn for_chat(&self, chat_id: &str) {
        *self.chat.lock().unwrap() = Some(chat_id.to_string());
    }
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
    // This PC's provider, or (a chat synced from another PC, where the same
    // server was added under another id) one here offering the same model.
    let found = s
        .providers
        .iter()
        .find(|p| p.id == pid && p.models.iter().any(|m| m.id == mid))
        .or_else(|| s.providers.iter().find(|p| p.enabled && !p.key.is_empty() && p.models.iter().any(|m| m.id == mid)));
    let p = match found {
        Some(p) => p,
        None if s.providers.iter().any(|p| p.id == pid) => return Err("That cloud model was removed. Pick another model.".into()),
        None => return Err("That cloud provider was removed. Pick another model.".into()),
    };
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
    let cookies = if p.cookies.is_empty() { String::new() } else { cipher.decrypt(&p.cookies).unwrap_or_default() };
    let ctx = m.ctx.max(4096);
    let target = Target {
        kind: p.kind,
        provider: p.name.clone(),
        base_url: p.base_url.trim_end_matches('/').to_string(),
        key,
        cookies,
        model: m.clone(),
        redact: s.redact,
        state: state.clone(),
        chat: Default::default(),
        proxy: proxy_of(p, cipher),
    };
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
        arch: Arch { n_layer: 1, n_kv_heads: 1, head_dim: 1, max_ctx: ctx, active_fraction: 1.0, kv_whole: 0, kv_sliding: 0, kv_whole_width: 0, swa_window: 0 },
        default_ctx: ctx,
        tools: true,
        variants: vec![Variant { quant: "cloud".into(), quality: "Cloud".into(), file: String::new(), url: String::new(), size: 0, sha256: String::new(), parts: Vec::new() }],
        ratings: Ratings::default(),
        vision: m.vision.then(|| VisionFile { file: String::new(), url: String::new(), size: 0, sha256: String::new() }),
        local: None,
    };
    let installed = InstalledModel { model_id: id.to_string(), quant: "cloud".into(), path: String::new(), size: 0, installed_at: 0, tps: None , prompt_tps: None};
    Ok((ep, spec, installed))
}

/// Tokens in text, estimated (cloud providers have no local counter).
pub fn estimate_tokens(text: &str) -> u32 {
    (text.chars().count() as f64 / 3.5).ceil() as u32
}

// ---------- personal details ----------

pub(crate) static PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        // Keys and passwords first: they can hold runs of digits the others would take.
        ("secret", crate::practice::yourcode::SECRET_TEXT.clone()),
        ("address", crate::outgoing::STREET.clone()),
        ("email", Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b").unwrap()),
        ("card number", Regex::new(r"\b(?:\d[ -]?){12,18}\d\b").unwrap()),
        ("SSN", Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap()),
        ("phone", Regex::new(r"(?:\+\d{1,3}[ .-]?)?\(?\b\d{3}\)?[ .-]?\d{3}[ .-]?\d{4}\b").unwrap()),
        // Other countries' numbers, written with their + code: +44 20 7946 0958.
        ("phone", Regex::new(r"\+\d{1,3}[ .-]?\(?\d{1,4}\)?(?:[ .-]?\d{2,4}){2,4}\b").unwrap()),
    ]
});

pub(crate) fn luhn(digits: &str) -> bool {
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
    /// The user's own names, as whole words.
    names: Option<Regex>,
}

impl Redactor {
    /// Also swaps these names (the user's own: full, first and last).
    pub fn with_names(names: &[String]) -> Redactor {
        let mut words: Vec<String> = names.iter().map(|n| n.trim().to_string()).filter(|n| n.chars().count() >= 3).collect();
        words.sort_by_key(|w| std::cmp::Reverse(w.len()));
        words.dedup();
        let names = if words.is_empty() {
            None
        } else {
            Regex::new(&format!(r"(?i)\b(?:{})\b", words.iter().map(|w| regex::escape(w)).collect::<Vec<_>>().join("|"))).ok()
        };
        Redactor { names, ..Default::default() }
    }

    /// For a request from this PC: the user's name from the profile too.
    pub fn for_state(state: &crate::AppState) -> Redactor {
        let name = state.cipher().ok().map(|c| crate::db::profile(&state.db.lock().unwrap(), &c).name).unwrap_or_default();
        let me = crate::outgoing::Me::new(&name, Vec::new(), "");
        let mut names = vec![name];
        names.extend(me.first);
        names.extend(me.last);
        // The Windows account name shows in paths (C:\Users\<name>\...) in
        // listings, command output and errors, whatever the profile says.
        names.extend(std::env::var("USERNAME").or_else(|_| std::env::var("USER")).ok().filter(|u| u.chars().count() >= 3));
        Redactor::with_names(&names)
    }

    pub fn redact(&mut self, text: &str) -> String {
        let mut out = text.to_string();
        if let Some(re) = self.names.clone() {
            out = self.swap(&out, "name", &re);
        }
        for (label, re) in PATTERNS.iter() {
            out = self.swap(&out, label, re);
        }
        out
    }

    fn swap(&mut self, text: &str, label: &'static str, re: &Regex) -> String {
        let mut next = String::with_capacity(text.len());
        let mut last = 0;
        for m in re.find_iter(text) {
            if label == "card number" && !luhn(m.as_str()) {
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
            next.push_str(&text[last..m.start()]);
            next.push_str(&placeholder);
            last = m.end();
        }
        next.push_str(&text[last..]);
        next
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
        // Earlier replies and their tool calls hold the real values again
        // (they were put back for the user), so they're swapped too. The
        // provider's own blocks (cloud_blocks) were never put back.
        for c in m.get_mut("tool_calls").and_then(Value::as_array_mut).into_iter().flatten() {
            if let Some(Value::String(a)) = c.pointer_mut("/function/arguments") {
                *a = r.redact(a);
            }
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

/// A client for one provider: through its own proxy when it has one.
fn via(state: &AppState, proxy: &str) -> Result<reqwest::Client, String> {
    net::external_client_via(state.settings().connectivity, Purpose::Cloud, false, Some(proxy).filter(|p| !p.is_empty()))
}

/// A provider's proxy, opened (empty: none).
fn proxy_of(p: &Provider, c: &Cipher) -> String {
    if p.proxy.is_empty() { String::new() } else { c.decrypt(&p.proxy).unwrap_or_default() }
}

pub fn media_client(state: &AppState, t: &MediaTarget) -> Result<reqwest::Client, String> {
    via(state, &t.proxy)
}

/// Where a picture, video or voice job goes.
pub struct MediaTarget {
    pub provider: String,
    pub base_url: String,
    key: String,
    pub model: CloudMedia,
    pub redact: bool,
    proxy: String,
}

impl MediaTarget {
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// A cloud media model's target, with the same checks as cloud chats
/// (Cloud level, provider on, key set, budget left).
pub fn media_target(state: &AppState, id: &str, cipher: &Cipher) -> Result<MediaTarget, String> {
    let (pid, mid) = parse_ref(id).ok_or("Unknown cloud model.")?;
    let (level, s, spent) = {
        let conn = state.db.lock().unwrap();
        (db::settings(&conn).connectivity, settings(&conn), spend(&conn))
    };
    if level != Connectivity::Cloud {
        return Err("Cloud models need the Cloud level. Switch to it in the status bar, or use a model on this PC.".into());
    }
    let p = s.providers.iter().find(|p| p.id == pid).ok_or("That cloud provider was removed.")?;
    if !p.enabled {
        return Err(format!("{} is turned off in Settings → Cloud models.", p.name));
    }
    if p.kind != Kind::Openai {
        return Err(format!("{} doesn't offer pictures, video or voices here.", p.name));
    }
    let m = p.media.iter().find(|m| m.id == mid).ok_or("That cloud model was removed.")?;
    if let Some(b) = s.budget {
        if spent.usd >= b {
            return Err(format!("This month's cloud budget (${b:.2}) is used up. Raise it in Settings → Cloud models."));
        }
    }
    let key = if p.key.is_empty() { String::new() } else { cipher.decrypt(&p.key)? };
    if key.is_empty() {
        return Err(format!("Add your {} API key in Settings → Cloud models.", p.name));
    }
    Ok(MediaTarget { provider: p.name.clone(), base_url: p.base_url.trim_end_matches('/').to_string(), key, model: m.clone(), redact: s.redact, proxy: proxy_of(p, cipher) })
}

/// Cloud media models usable right now (Cloud level, provider on, key set).
#[derive(Debug, Clone, Serialize)]
pub struct MediaChoice {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub kind: MediaKind,
    pub price: Option<f64>,
}

pub fn media_choices(conn: &Connection) -> Vec<MediaChoice> {
    if db::settings(conn).connectivity != Connectivity::Cloud {
        return Vec::new();
    }
    settings(conn)
        .providers
        .iter()
        .filter(|p| p.enabled && !p.key.is_empty() && p.kind == Kind::Openai)
        .flat_map(|p| p.media.iter().map(|m| MediaChoice { id: model_ref(&p.id, &m.id), name: m.name.clone(), provider: p.name.clone(), kind: m.kind, price: m.price }))
        .collect()
}

/// Adds a media job's estimated cost to this month's spend.
pub fn add_spend(conn: &Connection, usd: f64) {
    let mut s = spend(conn);
    s.usd += usd;
    s.requests += 1;
    let _ = db::set(conn, SPEND_KEY, &s);
}

/// OpenAI chat messages → Anthropic's system text, messages and tools.
fn to_anthropic(messages: &[Value], tools: Option<&Value>) -> (String, Vec<Value>, Vec<Value>) {
    let mut system = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    // Thinking blocks are replayed only for the turn in progress (a run of
    // tool calls); earlier turns keep their text and tool calls.
    // A message added mid-reply follows a tool result; the turn goes on, so it doesn't count.
    let last_user = (0..messages.len()).rev().find(|&i| messages[i]["role"] == "user" && (i == 0 || messages[i - 1]["role"] != "tool")).unwrap_or(0);
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
    let mut redactor = Redactor::for_state(&t.state);
    if t.redact {
        redact_messages(&mut messages, &mut redactor);
    }
    let client = via(&t.state, &t.proxy)?;
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

/// What one chat has cost (all its months), kept with the settings.
pub fn chat_spend_key(chat_id: &str) -> String {
    format!("cloud_chat:{chat_id}")
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
        // And the chat it was for.
        if let Some(chat) = t.chat.lock().unwrap().clone() {
            let key = chat_spend_key(&chat);
            let mut c: Spend = db::get(&conn, &key).unwrap_or_default();
            c.usd += cost;
            c.input_tokens += input as u64;
            c.output_tokens += output as u64;
            c.requests += 1;
            let _ = db::set(&conn, &key, &c);
        }
    }
    if let Some(app) = t.state.app.get() {
        use tauri::Emitter;
        app.emit("cloud:spent", json!({ "chat_id": t.chat.lock().unwrap().clone() })).ok();
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
        401 | 403 => format!("{who} didn't accept the API key ({status}). Check it in Settings → Cloud models; for a server you sign in to (like Open WebUI), press Sign in… there instead."),
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
    let mut req = client.post(format!("{}/chat/completions", t.base_url)).bearer_auth(&t.key);
    if !t.cookies.is_empty() {
        req = req.header("cookie", &t.cookies);
    }
    let resp = req
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

async fn list_models(p: &Provider, key: &str, cookies: &str, client: &reqwest::Client) -> Result<Vec<CloudModel>, String> {
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
            let mut req = client.get(format!("{base}/models")).bearer_auth(key);
            if !cookies.is_empty() {
                req = req.header("cookie", cookies);
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let to_login = resp.status().is_redirection() || resp.url().host_str().is_some_and(|h| h.contains("login.") || h.contains("microsoftonline"));
            if to_login {
                return Err(not_an_api(&p.name, true));
            }
            if !resp.status().is_success() {
                return Err(error_text(resp, &p.name).await);
            }
            let body = resp.text().await.map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_str(&body).map_err(|_| not_an_api(&p.name, body.trim_start().starts_with('<')))?;
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

/// A server that answered its model list with something other than its API:
/// a sign-in page (an app proxy in front of it) or the wrong address.
fn not_an_api(name: &str, web_page: bool) -> String {
    if web_page {
        format!("{name} answered with a web page (likely a sign-in page) instead of its API. Press Sign in… to sign in to it in the built-in browser, then choose its models again.")
    } else {
        format!("{name} didn't answer like an OpenAI-compatible API. Check the address: Open WebUI's ends in /api, LiteLLM's in /v1.")
    }
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
    media: Vec<CloudMedia>,
    /// Picture, video and voice models this provider offers.
    media_suggestions: Vec<CloudMedia>,
    share_pcs: bool,
    share_people: Vec<String>,
    /// Its requests go through a proxy of their own.
    has_proxy: bool,
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
    ProviderView {
        id: p.id.clone(),
        name: p.name.clone(),
        kind: p.kind,
        base_url: p.base_url.clone(),
        enabled: p.enabled,
        has_key: !p.key.is_empty(),
        models: p.models.clone(),
        media: p.media.clone(),
        media_suggestions: media_suggestions(&p.id),
        share_pcs: p.share_pcs,
        share_people: p.share_people.clone(),
        has_proxy: !p.proxy.is_empty(),
    }
}

#[tauri::command(async)]
pub fn cloud_view(state: AppStateRef) -> CloudView {
    let conn = state.db.lock().unwrap();
    let s = settings(&conn);
    let presets = presets().iter().filter(|p| !s.providers.iter().any(|q| q.id == p.id)).map(pview).collect();
    CloudView { providers: s.providers.iter().map(pview).collect(), presets, budget: s.budget, redact: s.redact, spend: spend(&conn) }
}

/// Adds a preset provider, or a custom OpenAI-compatible one.
#[tauri::command(async)]
pub fn add_cloud_provider(state: AppStateRef, preset: Option<String>, name: Option<String>, base_url: Option<String>) -> Result<CloudView, String> {
    state.cipher()?;
    let p = match preset {
        Some(id) => presets().into_iter().find(|p| p.id == id).ok_or("Unknown provider")?,
        None => {
            let url = clean_base(&base_url.unwrap_or_default());
            if !net::private_ok(&url) {
                return Err("Use an https:// address (or a local one).".into());
            }
            let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Custom provider".into());
            // The same server gets the same id on every PC, so a chat synced
            // from another PC finds its model here. The id stays in this
            // app (and its synced chats); it's never sent to the server.
            Provider { id: custom_id(&url), name, kind: Kind::Openai, base_url: url, enabled: false, key: String::new(), models: Vec::new(), media: Vec::new(), cookies: String::new(), share_pcs: false, share_people: Vec::new(), proxy: String::new() }
        }
    };
    let mut p = p;
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        // The same server again (another key): an id of its own.
        if p.id.starts_with("custom-") {
            let mut n = 2;
            while s.providers.iter().any(|q| q.id == p.id) {
                p.id = custom_id(&format!("{}#{n}", p.base_url));
                n += 1;
            }
        }
        if !s.providers.iter().any(|q| q.id == p.id) {
            s.providers.push(p.clone());
        }
        save(&conn, &s)?;
    }
    state.log("settings", &format!("Added the cloud provider {}", p.name));
    Ok(cloud_view(state))
}

#[tauri::command(async)]
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
#[tauri::command(async)]
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
    let cookies = if p.cookies.is_empty() { String::new() } else { c.decrypt(&p.cookies).unwrap_or_default() };
    let client = via(&state, &proxy_of(&p, &c))?;
    state.log("network", &format!("Asked {} which models it offers", p.name));
    list_models(&p, &key, &cookies, &client).await
}

/// Where a server's pages are, from its API address: Open WebUI's API is
/// under /api, LiteLLM's under /v1.
fn site_of(base_url: &str) -> Result<String, String> {
    let b = base_url.trim().trim_end_matches('/');
    let b = b.strip_suffix("/api").or_else(|| b.strip_suffix("/v1")).unwrap_or(b);
    let u = reqwest::Url::parse(&format!("{b}/")).map_err(|_| "That server's address isn't a web address.".to_string())?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("That server's address isn't a web address.".into());
    }
    Ok(u.to_string())
}

/// Signs in to a server in the built-in browser (Open WebUI, or one
/// behind a sign-in page): its pages open there, and once the user has
/// signed in, its session is used: Open WebUI's sign-in becomes the key,
/// and the site's cookies go with its API calls. Watches for ten minutes.
#[tauri::command(async)]
pub fn cloud_sign_in(app: tauri::AppHandle, state: AppStateRef, id: String) -> Result<String, String> {
    use tauri::Emitter;
    state.cipher()?;
    let p = settings(&state.db.lock().unwrap()).providers.into_iter().find(|p| p.id == id).ok_or("Unknown provider")?;
    if p.kind != Kind::Openai || presets().iter().any(|x| x.id == p.id) {
        return Err(format!("{} uses an API key: press Get a key… to sign in to your account and make one.", p.name));
    }
    let site = site_of(&p.base_url)?;
    app.emit("viewer:open", json!({ "url": site })).map_err(|e| e.to_string())?;
    state.log("settings", &format!("Opened {} in the built-in browser to sign in", p.name));
    let state = state.inner().clone();
    let at = site.clone();
    tauri::async_runtime::spawn(async move { watch_sign_in(app, state, id, at).await });
    Ok(site)
}

/// Waits for the sign-in in the viewer, trying the session each time the
/// site's cookies change, until its model list answers.
async fn watch_sign_in(app: tauri::AppHandle, state: Arc<AppState>, id: String, site: String) {
    use tauri::Emitter;
    let Ok(url) = tauri::Url::parse(&site) else { return };
    let until = std::time::Instant::now() + Duration::from_secs(600);
    let mut tried = String::new();
    while std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_secs(2)).await;
        // Any web viewer: they share one profile, so one sign-in.
        let Some(w) = crate::viewer::all(&app).into_iter().next() else { continue };
        let u = url.clone();
        // Never on the main thread: reading cookies there waits forever (WebView2).
        let Ok(Ok(cookies)) = tokio::task::spawn_blocking(move || w.cookies_for_url(u)).await else { continue };
        let header = cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
        if header.is_empty() || header == tried {
            continue;
        }
        tried = header.clone();
        let (Ok(c), Some(p)) = (state.cipher(), settings(&state.db.lock().unwrap()).providers.into_iter().find(|p| p.id == id)) else { return };
        // Open WebUI keeps its sign-in in "token"; otherwise the key saved before.
        let token = cookies.iter().find(|c| c.name() == "token").map(|c| c.value().to_string());
        let key = match token.clone() {
            Some(t) => t,
            None if !p.key.is_empty() => c.decrypt(&p.key).unwrap_or_default(),
            None => continue,
        };
        let Ok(http) = via(&state, &proxy_of(&p, &c)) else { return };
        if list_models(&p, &key, &header, &http).await.is_err() {
            continue;
        }
        let saved = {
            let conn = state.db.lock().unwrap();
            let mut s = settings(&conn);
            match s.providers.iter_mut().find(|x| x.id == id) {
                Some(x) => {
                    x.key = c.encrypt(&key);
                    // Its token alone is enough for Open WebUI.
                    x.cookies = if token.is_some() { String::new() } else { c.encrypt(&header) };
                    x.enabled = true;
                    save(&conn, &s).is_ok()
                }
                None => false,
            }
        };
        if saved {
            state.log("settings", &format!("Signed in to {} in the built-in browser; its session is kept encrypted", p.name));
            app.emit("cloud:signed_in", json!({ "id": id, "name": p.name })).ok();
        }
        return;
    }
}

/// What a chat has cost on cloud models, with this month's total and the budget.
#[derive(Serialize)]
pub struct ChatSpend {
    chat: Spend,
    month: Spend,
    budget: Option<f64>,
}

#[tauri::command(async)]
pub fn chat_cloud_spend(state: AppStateRef, chat_id: String) -> ChatSpend {
    let conn = state.db.lock().unwrap();
    ChatSpend { chat: db::get(&conn, &chat_spend_key(&chat_id)).unwrap_or_default(), month: spend(&conn), budget: settings(&conn).budget }
}

/// Which of the provider's models appear in the model lists.
#[tauri::command(async)]
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

/// The picture, video and voice models turned on for a provider.
#[tauri::command(async)]
pub fn set_cloud_media(state: AppStateRef, id: String, media: Vec<CloudMedia>) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        p.media = media.into_iter().filter(|m| !m.id.trim().is_empty()).collect();
        save(&conn, &s)?;
    }
    Ok(cloud_view(state))
}

#[tauri::command(async)]
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
    pub name: String,
    pub provider: String,
    pub vision: bool,
    /// US dollars per million tokens in and out, if known.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
}

#[tauri::command(async)]
pub fn cloud_choices(state: AppStateRef) -> Vec<CloudChoice> {
    choices(&state.db.lock().unwrap())
}

/// A server's provider id, from its address (the same on every PC).
pub fn custom_id(base_url: &str) -> String {
    use sha2::{Digest, Sha256};
    let url = base_url.trim().trim_end_matches('/').to_lowercase();
    format!("custom-{}", &hex::encode(Sha256::digest(url.as_bytes()))[..8])
}

/// Which provider here a cloud model id is from: the same id, or (a chat
/// synced from another PC) one offering the same model.
pub fn provider_of<'a>(s: &'a CloudSettings, id: &str) -> Option<&'a Provider> {
    let (pid, mid) = parse_ref(id)?;
    s.providers
        .iter()
        .find(|p| p.id == pid && p.models.iter().any(|m| m.id == mid))
        .or_else(|| s.providers.iter().find(|p| p.enabled && !p.key.is_empty() && p.models.iter().any(|m| m.id == mid)))
}

/// A fingerprint of a provider's address and key: two PCs with the same one
/// use the same account. It can't be turned back into the key, and it's
/// only told to this user's own paired PCs, inside their encrypted link.
pub fn key_print(c: &Cipher, p: &Provider) -> Option<String> {
    use sha2::{Digest, Sha256};
    let key = c.decrypt(&p.key).ok().filter(|k| !k.trim().is_empty())?;
    let url = p.base_url.trim().trim_end_matches('/').to_lowercase();
    let mut h = Sha256::new();
    h.update(b"sulcusai-key-print\0");
    h.update(url.as_bytes());
    h.update(b"\0");
    h.update(key.trim().as_bytes());
    Some(hex::encode(h.finalize())[..20].to_string())
}

/// The fingerprints of the providers whose chats go to this user's other PCs.
pub fn shared_prints(conn: &Connection, c: &Cipher) -> Vec<String> {
    settings(conn).providers.iter().filter(|p| p.share_pcs && p.enabled).filter_map(|p| key_print(c, p)).collect()
}

/// Chats made with a cloud model that another PC (with these fingerprints)
/// doesn't get: its provider's chats stay on this PC, or that PC doesn't
/// use the same key.
pub fn withheld_chats(conn: &Connection, c: &Cipher, theirs: &[String]) -> std::collections::HashSet<String> {
    let s = settings(conn);
    let Ok(mut stmt) = conn.prepare("SELECT id, model_id FROM chats WHERE model_id LIKE 'cloud:%'") else { return Default::default() };
    let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default();
    rows.into_iter()
        .filter(|(_, model)| match provider_of(&s, model) {
            Some(p) => !(p.share_pcs && key_print(c, p).is_some_and(|k| theirs.contains(&k))),
            None => true,
        })
        .map(|(id, _)| id)
        .collect()
}

/// The chats made with a provider's models.
pub fn chats_of(conn: &Connection, provider: &Provider) -> Vec<String> {
    let s = CloudSettings { providers: vec![provider.clone()], ..Default::default() };
    let Ok(mut stmt) = conn.prepare("SELECT id, model_id FROM chats WHERE model_id LIKE 'cloud:%'") else { return Vec::new() };
    let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default();
    rows.into_iter().filter(|(_, m)| provider_of(&s, m).is_some_and(|p| p.id == provider.id)).map(|(id, _)| id).collect()
}

/// The people a chat goes to because of its cloud model's provider.
pub fn people_for(conn: &Connection, model_id: Option<&str>) -> Vec<String> {
    let Some(m) = model_id.filter(|m| is_cloud(m)) else { return Vec::new() };
    provider_of(&settings(conn), m).map(|p| p.share_people.clone()).unwrap_or_default()
}

/// What a proxy address must look like: socks5://, socks5h:// (names looked
/// up through the proxy, needed for names only the VPN knows), http:// or
/// https://, with a host and a port.
pub fn proxy_problem(proxy: &str) -> Option<&'static str> {
    let Ok(u) = reqwest::Url::parse(proxy.trim()) else { return Some("Write the proxy like socks5h://127.0.0.1:1080.") };
    if !matches!(u.scheme(), "socks5" | "socks5h" | "http" | "https") {
        return Some("The proxy must start with socks5h://, socks5://, http:// or https://.");
    }
    if u.host_str().is_none_or(str::is_empty) || u.port().is_none() {
        return Some("Give the proxy's address and port, like socks5h://127.0.0.1:1080.");
    }
    None
}

/// Sends one provider's requests through a proxy (empty: directly).
#[tauri::command(async)]
pub fn set_cloud_proxy(state: AppStateRef, id: String, proxy: String) -> Result<CloudView, String> {
    let c = state.cipher()?;
    let proxy = proxy.trim().to_string();
    if !proxy.is_empty() {
        if let Some(why) = proxy_problem(&proxy) {
            return Err(why.into());
        }
    }
    let name = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        p.proxy = if proxy.is_empty() { String::new() } else { c.encrypt(&proxy) };
        let name = p.name.clone();
        save(&conn, &s)?;
        name
    };
    state.log("settings", &format!("{name}: requests {}", if proxy.is_empty() { "go directly" } else { "go through its own proxy" }));
    Ok(cloud_view(state))
}

/// Where a provider's chats go: this user's other PCs with the same key,
/// and people linked in People.
#[tauri::command(async)]
pub fn set_cloud_sharing(state: AppStateRef, id: String, share_pcs: bool, share_people: Vec<String>) -> Result<CloudView, String> {
    let c = state.cipher()?;
    let (p, chats, was) = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        let was = p.share_pcs;
        p.share_pcs = share_pcs;
        p.share_people = share_people;
        let p = p.clone();
        save(&conn, &s)?;
        let chats = chats_of(&conn, &p);
        (p, chats, was)
    };
    // Turned on: chats held back from the other PCs go at the next sync.
    if share_pcs && !was {
        let _ = crate::sync::track::resend_chats(&state.db.lock().unwrap(), &chats);
        crate::sync::wake();
    }
    for chat in &chats {
        crate::sync::people::share::follow_provider(&state, &c, chat);
    }
    state.log(
        "settings",
        &format!("{}: chats {} the other PCs with its key{}", p.name, if share_pcs { "go to" } else { "stay off" }, if p.share_people.is_empty() { String::new() } else { format!(", and are shared with {} people", p.share_people.len()) }),
    );
    Ok(cloud_view(state))
}

/// The model a cloud id names, here: the same one, or the same model from a
/// provider this PC has under another id (a chat synced from another PC).
pub fn resolve(conn: &Connection, id: &str) -> Option<String> {
    let all = choices(conn);
    if all.iter().any(|c| c.id == id) {
        return Some(id.to_string());
    }
    let (_, model) = parse_ref(id)?;
    all.into_iter().find(|c| parse_ref(&c.id).is_some_and(|(_, m)| m == model)).map(|c| c.id)
}

pub fn choices(conn: &Connection) -> Vec<CloudChoice> {
    settings(conn)
        .providers
        .iter()
        .filter(|p| p.enabled && !p.key.is_empty())
        .flat_map(|p| {
            p.models.iter().map(move |m| CloudChoice { id: model_ref(&p.id, &m.id), name: m.name.clone(), provider: p.name.clone(), vision: m.vision, price_in: m.price_in, price_out: m.price_out })
        })
        .collect()
}

/// For a test against a real provider set up in the app: the models it
/// offers, and a one-word reply from each model picked. Nothing secret
/// (key, cookies, proxy, address) goes in the report.
#[cfg(test)]
pub(crate) async fn check_provider(p: &Provider, c: &Cipher) -> String {
    let mut report = format!("{} ({}, {}): ", p.name, if p.base_url.starts_with("https://") { "https" } else { "plain http" }, if p.proxy.is_empty() { "direct" } else { "through its proxy" });
    let key = match c.decrypt(&p.key) {
        Ok(k) => k,
        Err(_) => return report + "its key couldn't be opened",
    };
    let cookies = if p.cookies.is_empty() { String::new() } else { c.decrypt(&p.cookies).unwrap_or_default() };
    let proxy = proxy_of(p, c);
    let client = match net::external_client_via(Connectivity::Cloud, Purpose::Cloud, false, Some(proxy.as_str()).filter(|x| !x.is_empty())) {
        Ok(c) => c,
        Err(e) => return report + &e,
    };
    let started = std::time::Instant::now();
    match list_models(p, &key, &cookies, &client).await {
        Ok(models) => report += &format!("offers {} models in {} ms; ", models.len(), started.elapsed().as_millis()),
        Err(e) => return report + &format!("listing its models failed after {} ms: {e}", started.elapsed().as_millis()),
    }
    if p.kind != Kind::Openai {
        return report;
    }
    let base = p.base_url.trim_end_matches('/');
    for m in &p.models {
        let t = std::time::Instant::now();
        let body = json!({ "model": m.id, "messages": [{ "role": "user", "content": "Reply with only the word OK." }], "max_tokens": 200, "stream": false });
        let mut req = client.post(format!("{base}/chat/completions")).bearer_auth(&key).json(&body).timeout(Duration::from_secs(120));
        if !cookies.is_empty() {
            req = req.header("cookie", &cookies);
        }
        let answer = match req.send().await {
            Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                Ok(v) => format!("\u{201c}{}\u{201d}", v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("(empty)").trim().chars().take(60).collect::<String>()),
                Err(e) => format!("unreadable reply ({e})"),
            },
            Ok(r) => format!("error {}", r.status()),
            Err(e) => format!("failed ({e})"),
        };
        report += &format!("\n  {}: {answer} in {} ms", m.name, t.elapsed().as_millis());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_from_other_countries_are_phone_numbers() {
        let mut r = Redactor::default();
        let out = r.redact("Ring +44 20 7946 0958 or +49 30 1234 5678 after 6.");
        assert!(!out.contains("7946") && !out.contains("1234 5678"), "{out}");
        assert!(r.redact("version 2.5.8 of 12 files").contains("2.5.8"));
    }

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
    fn names_keys_addresses_and_earlier_replies_are_redacted_too() {
        let mut r = Redactor::with_names(&["Isaac Lehman".into(), "Isaac".into(), "Lehman".into()]);
        let out = r.redact("Isaac Lehman lives at 1234 North Main Street; his key is sk-abc123DEF456ghi789JKL012mno. Isaacson isn't him.");
        assert!(!out.contains("Lehman") && !out.contains("Main Street") && !out.contains("sk-abc"), "{out}");
        assert!(out.contains("[name 1]") && out.contains("[address 1]") && out.contains("[secret 1]"), "{out}");
        assert!(out.contains("Isaacson"), "only whole words: {out}");
        assert_eq!(r.restore("Hi [name 1]!"), "Hi Isaac Lehman!");
        // Earlier replies and tool calls are swapped too, so real values
        // put back for the user don't go out again.
        let mut messages = vec![json!({ "role": "assistant", "content": "Sent to jo@example.com.", "tool_calls": [{ "id": "t1", "type": "function", "function": { "name": "email_send", "arguments": "{\"to\":\"jo@example.com\"}" } }] })];
        let mut r = Redactor::default();
        redact_messages(&mut messages, &mut r);
        assert_eq!(messages[0]["content"], "Sent to [email 1].");
        assert_eq!(messages[0]["tool_calls"][0]["function"]["arguments"], "{\"to\":\"[email 1]\"}");
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
    fn proxy_addresses_are_checked_and_carry_the_providers_requests() {
        // A provider whose address only the proxy can reach (as a server
        // inside a company network is reached only through the VPN).
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            use axum::{routing::get, Json, Router};
            let seen: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
            let s = seen.clone();
            let app = Router::new().route(
                "/v1/models",
                get(move |uri: axum::http::Uri| {
                    let s = s.clone();
                    async move {
                        s.lock().unwrap().push(uri.to_string());
                        Json(serde_json::json!({ "data": [{ "id": "gpt-4o" }] }))
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(listener, app).await.ok() });
            let p = Provider { id: "custom-vpn".into(), name: "LiteLLM".into(), kind: Kind::Openai, base_url: "http://llm.corp.invalid:4000/v1".into(), enabled: true, key: String::new(), models: vec![], media: vec![], cookies: String::new(), share_pcs: false, share_people: vec![], proxy: String::new() };
            let direct = net::external_client_via(Connectivity::Cloud, Purpose::Cloud, false, None).unwrap();
            assert!(list_models(&p, "k", "", &direct).await.is_err(), "not reachable directly");
            let proxied = net::external_client_via(Connectivity::Cloud, Purpose::Cloud, false, Some(&format!("http://127.0.0.1:{port}"))).unwrap();
            let models = list_models(&p, "k", "", &proxied).await.unwrap();
            assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-4o"]);
            assert!(seen.lock().unwrap()[0].contains("llm.corp.invalid"), "the proxy was asked for the provider's address");
        });
    }

    #[test]
    fn pasted_endpoints_come_off_a_servers_address() {
        assert_eq!(clean_base("https://litellm.example.edu/v1/models"), "https://litellm.example.edu/v1");
        assert_eq!(clean_base("https://x.example/v1/chat/completions/"), "https://x.example/v1");
        assert_eq!(clean_base(" https://x.example/api/ "), "https://x.example/api");
        assert_eq!(clean_base("https://api.anthropic.com"), "https://api.anthropic.com");
        assert_eq!(clean_base("https://litellm.example.edu/v1/models/v1"), "https://litellm.example.edu/v1");
        assert_eq!(clean_base("https://litellm.example.edu/models/v1"), "https://litellm.example.edu/v1");
        assert_eq!(clean_base("https://litellm.example.edu/v1"), "https://litellm.example.edu/v1");
    }

    #[test]
    fn proxy_addresses_are_checked() {
        assert!(proxy_problem("socks5h://127.0.0.1:11080").is_none());
        assert!(proxy_problem("http://proxy.corp.example:3128").is_none());
        assert!(proxy_problem("127.0.0.1:1080").is_some(), "the kind is needed");
        assert!(proxy_problem("socks5h://127.0.0.1").is_some(), "and the port");
        assert!(proxy_problem("ftp://x:21").is_some());
    }

    #[test]
    fn model_refs_round_trip() {
        let id = model_ref("openrouter", "meta-llama/llama-4:free");
        assert_eq!(parse_ref(&id), Some(("openrouter", "meta-llama/llama-4:free")));
        assert!(is_cloud(&id) && !is_cloud("qwen3-1.7b"));
    }

    /// A chat synced from the laptop names the laptop's provider id: the same
    /// server here is found by its model, and its id is the same anyway now.
    #[test]
    fn a_synced_chats_model_is_found_on_this_pc() {
        assert_eq!(custom_id("https://llm.example.com/v1/"), custom_id("https://LLM.example.com/v1"));
        assert_ne!(custom_id("https://llm.example.com/v1"), custom_id("https://other.example.com/v1"));
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_for_test(&conn);
        let model = CloudModel { id: "gpt-4o".into(), name: "GPT-4o".into(), ..Default::default() };
        let here = Provider {
            id: custom_id("https://llm.example.com/v1"),
            name: "LiteLLM".into(),
            kind: Kind::Openai,
            base_url: "https://llm.example.com/v1".into(),
            enabled: true,
            key: "sealed".into(),
            models: vec![model],
            media: vec![],
            cookies: String::new(),
            share_pcs: false,
            share_people: vec![],
            proxy: String::new(),
        };
        save(&conn, &CloudSettings { providers: vec![here.clone()], ..Default::default() }).unwrap();
        let mine = model_ref(&here.id, "gpt-4o");
        assert_eq!(resolve(&conn, &mine).as_deref(), Some(mine.as_str()));
        // The laptop's old random id: the same model here.
        assert_eq!(resolve(&conn, "cloud:custom-1a2b3c4d:gpt-4o").as_deref(), Some(mine.as_str()));
        assert_eq!(resolve(&conn, "cloud:custom-1a2b3c4d:claude-opus"), None, "a model this PC doesn't have");
        assert_eq!(provider_of(&settings(&conn), "cloud:custom-1a2b3c4d:gpt-4o").map(|p| p.id.as_str()), Some(here.id.as_str()));
    }
}

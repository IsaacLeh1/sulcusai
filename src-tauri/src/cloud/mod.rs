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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The OpenAI chat completions API (OpenAI, Gemini, OpenRouter, others).
    #[default]
    Openai,
    /// Anthropic's Messages API.
    Anthropic,
    /// Azure OpenAI and Azure AI Foundry: OpenAI's API with deployments
    /// for models, an api-version, and its own key header or Entra sign-in.
    Azure,
    /// AWS Bedrock's Converse API, signed with AWS keys.
    Bedrock,
    /// Google Vertex AI: Gemini through its OpenAI-compatible endpoint and
    /// Claude through Anthropic's API, with Google sign-in tokens.
    Vertex,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    #[serde(default)]
    pub ent: enterprise::Ent,
    /// Extra headers sent with every request (a gateway's org or project
    /// ids): a JSON object, encrypted, empty when none.
    #[serde(default)]
    pub headers: String,
    /// A company CA certificate (PEM) trusted for this provider only, for a
    /// gateway that inspects TLS with its own certificate.
    #[serde(default)]
    pub ca_cert: String,
}

impl Provider {
    /// A key, or a sign-in that keeps none here (Entra, an AWS profile,
    /// Google's application default credentials).
    pub fn has_credentials(&self) -> bool {
        !self.key.is_empty() || self.ent.keyless(self.kind) || (self.id == crate::policy::GATEWAY_ID && crate::policy::get().gateway.is_some_and(|g| g.keyless))
    }
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
    /// Of the input, how much was read from the provider's prompt cache.
    pub cached_tokens: u64,
    pub output_tokens: u64,
    pub requests: u64,
}

pub fn settings(conn: &Connection) -> CloudSettings {
    let mut s: CloudSettings = db::get(conn, KEY).unwrap_or_default();
    // An address pasted with its endpoint on the end ("…/v1/models").
    for p in &mut s.providers {
        p.base_url = clean_base(&p.base_url);
    }
    let pol = crate::policy::get();
    if pol.force_redact {
        s.redact = true;
    }
    // The company gateway is always listed; only its key and models are the user's.
    if let Some(g) = &pol.gateway {
        let at = match s.providers.iter().position(|p| p.id == crate::policy::GATEWAY_ID) {
            Some(i) => i,
            None => {
                s.providers.insert(0, Provider { id: crate::policy::GATEWAY_ID.into(), ..Default::default() });
                0
            }
        };
        let p = &mut s.providers[at];
        p.name = if g.name.trim().is_empty() { "Company gateway".into() } else { g.name.clone() };
        p.kind = g.kind;
        p.base_url = clean_base(&g.base_url);
        p.ca_cert = g.ca_cert.clone();
    } else {
        s.providers.retain(|p| p.id != crate::policy::GATEWAY_ID);
    }
    s
}

fn policy_refuses(p: &Provider, model: &str, spent: f64) -> Option<String> {
    crate::policy::get().refuse(&p.id, p.kind, model, spent)
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
    let p = |id: &str, name: &str, kind, url: &str| Provider { id: id.into(), name: name.into(), kind, base_url: url.into(), ..Default::default() };
    vec![
        p("anthropic", "Anthropic (Claude)", Kind::Anthropic, "https://api.anthropic.com"),
        p("openai", "OpenAI", Kind::Openai, "https://api.openai.com/v1"),
        p("gemini", "Google Gemini", Kind::Openai, "https://generativelanguage.googleapis.com/v1beta/openai"),
        p("openrouter", "OpenRouter", Kind::Openai, "https://openrouter.ai/api/v1"),
        p("mistral", "Mistral", Kind::Openai, "https://api.mistral.ai/v1"),
        p("groq", "Groq", Kind::Openai, "https://api.groq.com/openai/v1"),
        p("together", "Together AI", Kind::Openai, "https://api.together.xyz/v1"),
        p("deepseek", "DeepSeek", Kind::Openai, "https://api.deepseek.com/v1"),
        p("xai", "xAI (Grok)", Kind::Openai, "https://api.x.ai/v1"),
        p("cohere", "Cohere", Kind::Openai, "https://api.cohere.ai/compatibility/v1"),
        p("fireworks", "Fireworks AI", Kind::Openai, "https://api.fireworks.ai/inference/v1"),
        // Addresses are the company's own resource or region, set after adding.
        p("azure", "Azure OpenAI / AI Foundry", Kind::Azure, ""),
        p("bedrock", "AWS Bedrock", Kind::Bedrock, ""),
        p("vertex", "Google Vertex AI", Kind::Vertex, ""),
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
    wire: Wire,
    pub provider_id: String,
    ent: enterprise::Ent,
}

impl Target {
    pub fn for_chat(&self, chat_id: &str) {
        *self.chat.lock().unwrap() = Some(chat_id.to_string());
    }

    /// A status line for the chat this reply is for (trying again, and so on).
    fn tell(&self, mut payload: Value) {
        use tauri::Emitter;
        let (Some(app), Some(chat)) = (self.state.app.get(), self.chat.lock().unwrap().clone()) else { return };
        payload["chat_id"] = json!(chat);
        app.emit("chat:status", payload).ok();
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
        .or_else(|| s.providers.iter().find(|p| p.enabled && p.has_credentials() && p.models.iter().any(|m| m.id == mid)));
    let p = match found {
        Some(p) => p,
        None if s.providers.iter().any(|p| p.id == pid) => return Err("That cloud model was removed. Pick another model.".into()),
        None => return Err("That cloud provider was removed. Pick another model.".into()),
    };
    if !p.enabled {
        return Err(format!("{} is turned off in Settings → Cloud models.", p.name));
    }
    let m = p.models.iter().find(|m| m.id == mid).ok_or("That cloud model was removed. Pick another model.")?;
    if let Some(why) = policy_refuses(p, mid, spent.usd).or_else(|| enterprise::problem(p)) {
        return Err(why);
    }
    if let Some(b) = s.budget {
        if spent.usd >= b {
            return Err(format!("This month's cloud budget (${b:.2}) is used up. Raise it in Settings → Cloud models, or pick a model on this PC."));
        }
    }
    let key = if p.key.is_empty() { String::new() } else { cipher.decrypt(&p.key)? };
    if key.is_empty() && !p.has_credentials() {
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
        wire: wire_of(p, cipher),
        provider_id: p.id.clone(),
        ent: p.ent.clone(),
    };
    let mut extra = crate::advanced::get(state).request_extra();
    crate::presets::apply_sampling(&mut extra, &crate::presets::get(&state.db.lock().unwrap(), id));
    let ep = Endpoint { port: 0, key: String::new(), ctx, extra, cloud: Some(Arc::new(target)) };
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
        variants: vec![Variant { quant: "cloud".into(), quality: "Cloud".into(), file: String::new(), url: String::new(), size: 0, sha256: String::new(), parts: Vec::new(), mtp: false }],
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

/// How one provider's requests leave this PC: its own proxy, a company CA
/// it trusts, and headers sent with each request (all opened).
#[derive(Clone, Default)]
pub(crate) struct Wire {
    proxy: String,
    ca: String,
    headers: Vec<(String, String)>,
}

impl Wire {
    fn add(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        req
    }
}

fn via(state: &AppState, w: &Wire) -> Result<reqwest::Client, String> {
    net::external_client_ca(state.settings().connectivity, Purpose::Cloud, Some(w.proxy.as_str()).filter(|p| !p.is_empty()), Some(w.ca.as_str()).filter(|c| !c.trim().is_empty()))
}

fn wire_of(p: &Provider, c: &Cipher) -> Wire {
    let open = |s: &str| if s.is_empty() { String::new() } else { c.decrypt(s).unwrap_or_default() };
    let headers = serde_json::from_str::<serde_json::Map<String, Value>>(&open(&p.headers))
        .map(|m| m.into_iter().filter_map(|(k, v)| Some((k, v.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let mut w = Wire { proxy: open(&p.proxy), ca: p.ca_cert.clone(), headers };
    if p.id == crate::policy::GATEWAY_ID {
        if let Some(g) = crate::policy::get().gateway {
            w.headers.extend(g.headers);
            if !g.proxy.trim().is_empty() {
                w.proxy = g.proxy;
            }
        }
    }
    w
}

pub fn media_client(state: &AppState, t: &MediaTarget) -> Result<reqwest::Client, String> {
    via(state, &t.wire)
}

/// The provider's extra headers, for media requests.
pub fn media_headers(t: &MediaTarget) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    for (k, v) in &t.wire.headers {
        if let (Ok(k), Ok(v)) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_str(v)) {
            h.insert(k, v);
        }
    }
    h
}

/// Where a picture, video or voice job goes.
pub struct MediaTarget {
    pub provider: String,
    pub base_url: String,
    key: String,
    pub model: CloudMedia,
    pub redact: bool,
    wire: Wire,
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
    if let Some(why) = policy_refuses(p, mid, spent.usd) {
        return Err(why);
    }
    if let Some(b) = s.budget {
        if spent.usd >= b {
            return Err(format!("This month's cloud budget (${b:.2}) is used up. Raise it in Settings → Cloud models."));
        }
    }
    let key = if p.key.is_empty() { String::new() } else { cipher.decrypt(&p.key)? };
    if key.is_empty() {
        return Err(format!("Add your {} API key in Settings → Cloud models.", p.name));
    }
    Ok(MediaTarget { provider: p.name.clone(), base_url: p.base_url.trim_end_matches('/').to_string(), key, model: m.clone(), redact: s.redact, wire: wire_of(p, cipher) })
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
        .filter(|p| p.enabled && p.has_credentials() && p.kind == Kind::Openai && crate::policy::get().provider_ok(&p.id, p.kind))
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
    let client = via(&t.state, &t.wire)?;
    let started = std::time::Instant::now();
    let mut out = {
        // Set once anything is shown: a new try after that would show it twice.
        let began = AtomicBool::new(false);
        let mut restore = |d: Delta| {
            began.store(true, Ordering::Relaxed);
            on_delta(match d {
                Delta::Content(s) => Delta::Content(redactor.restore(&s)),
                Delta::Thinking(s) => Delta::Thinking(redactor.restore(&s)),
            })
        };
        let mut tries = 0;
        let mut renewed = false;
        loop {
            let attempt = match t.kind {
                Kind::Anthropic => anthropic_stream(&client, t, extra, &messages, tools, cancel, &mut restore).await,
                Kind::Vertex if enterprise::vertex_claude(&t.model.id) => anthropic_stream(&client, t, extra, &messages, tools, cancel, &mut restore).await,
                Kind::Bedrock => bedrock::stream(&client, t, extra, &messages, tools, cancel, &mut restore).await,
                Kind::Openai | Kind::Azure | Kind::Vertex => openai_stream(&client, t, extra, &messages, tools, cancel, &mut restore).await,
            };
            let f = match attempt {
                Ok(out) => break out,
                Err(f) => f,
            };
            if !renewed && fresh_token_helps(&f, t.kind, &t.ent, began.load(Ordering::Relaxed)) {
                renewed = true;
                enterprise::forget(&t.provider_id);
                t.state.log("network", &format!("{} refused the sign-in token; fetching a new one and trying once more", t.provider));
                continue;
            }
            let Some(wait) = next_wait(&f, tries, began.load(Ordering::Relaxed)) else { return Err(f.message) };
            tries += 1;
            let secs = wait.as_secs().max(1);
            t.state.log("network", &format!("{} didn't answer this time ({}); trying again in {secs} s ({tries} of {RETRIES})", t.provider, f.message));
            t.tell(json!({ "status": "retrying", "provider": t.provider, "seconds": secs, "attempt": tries, "of": RETRIES }));
            if !pause(wait, cancel).await {
                let mut stopped = empty();
                stopped.cancelled = true;
                break stopped;
            }
        }
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
    record(t, &out, redactor.used());
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

/// What a reply cost. Cached input is cheaper: a tenth of the price for
/// Anthropic; OpenAI-style services discount it by half to nine tenths
/// depending on the model, so half is used (an estimate that never
/// undercounts the budget). Writing to Anthropic's cache costs a quarter more.
fn cost(kind: Kind, model: &CloudModel, input: u32, read: u32, write: u32, output: u32) -> f64 {
    let (read, write) = (read.min(input), write.min(input.saturating_sub(read.min(input))));
    let fresh = input - read - write;
    let (read_rate, write_rate) = match kind {
        Kind::Anthropic | Kind::Bedrock | Kind::Vertex => (0.1, 1.25),
        Kind::Openai | Kind::Azure => (0.5, 1.0),
    };
    let pin = model.price_in.unwrap_or(0.0);
    (fresh as f64 + read as f64 * read_rate + write as f64 * write_rate) * pin / 1e6 + output as f64 * model.price_out.unwrap_or(0.0) / 1e6
}

fn record(t: &Target, out: &Finished, redacted: usize) {
    let (input, output) = (out.prompt_tokens.unwrap_or(0), out.completion_tokens.unwrap_or(0));
    let read = out.cache_read.unwrap_or(0).min(input);
    let cost = cost(t.kind, &t.model, input, read, out.cache_write.unwrap_or(0), output);
    {
        let conn = t.state.db.lock().unwrap();
        let mut s = spend(&conn);
        s.usd += cost;
        s.input_tokens += input as u64;
        s.cached_tokens += read as u64;
        s.output_tokens += output as u64;
        s.requests += 1;
        let _ = db::set(&conn, SPEND_KEY, &s);
        // And the chat it was for.
        if let Some(chat) = t.chat.lock().unwrap().clone() {
            let key = chat_spend_key(&chat);
            let mut c: Spend = db::get(&conn, &key).unwrap_or_default();
            c.usd += cost;
            c.input_tokens += input as u64;
            c.cached_tokens += read as u64;
            c.output_tokens += output as u64;
            c.requests += 1;
            let _ = db::set(&conn, &key, &c);
        }
        let one = Spend { usd: cost, input_tokens: input as u64, cached_tokens: read as u64, output_tokens: output as u64, requests: 1, ..Default::default() };
        usage::add(&conn, &t.provider, t.chat.lock().unwrap().as_deref(), &one);
    }
    if let Some(app) = t.state.app.get() {
        use tauri::Emitter;
        app.emit("cloud:spent", json!({ "chat_id": t.chat.lock().unwrap().clone() })).ok();
    }
    let priced = if t.model.price_in.is_some() { format!(", about ${cost:.4}") } else { String::new() };
    let hidden = if redacted > 0 { format!("; {redacted} personal detail(s) replaced with placeholders") } else { String::new() };
    let cached = if read > 0 { format!("; {read} of them from the cache") } else { String::new() };
    t.state.log("network", &format!("Sent a chat to {} ({}): {input} tokens in, {output} out{priced}{cached}{hidden}", t.provider, t.model.name));
}

/// Marks where Anthropic may cache the request: the end of the tools, the
/// end of the instructions, and the last two messages (the newest, and the
/// one before it so the previous step's cache is found again). Four marks,
/// the most the API takes. Thinking blocks and empty text can't carry a mark,
/// so the last block that can is used. Requests under the model's minimum
/// (about 1,000 to 4,000 tokens) simply aren't cached; nothing fails.
fn mark_cache(system: &str, msgs: &mut [Value], tools: &mut [Value]) -> Value {
    let mark = json!({ "type": "ephemeral" });
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = mark.clone();
    }
    let n = msgs.len();
    for m in msgs.iter_mut().skip(n.saturating_sub(2)) {
        let markable = m["content"].as_array_mut().and_then(|blocks| {
            blocks.iter_mut().rev().find(|b| match b["type"].as_str() {
                Some("text") => b["text"].as_str().is_some_and(|t| !t.is_empty()),
                Some("image" | "tool_use" | "tool_result" | "document") => true,
                _ => false,
            })
        });
        if let Some(b) = markable {
            b["cache_control"] = mark.clone();
        }
    }
    if system.is_empty() {
        Value::Null
    } else {
        json!([{ "type": "text", "text": system, "cache_control": mark }])
    }
}

// ---------- trying again ----------

/// How many more tries a busy or briefly unreachable provider gets.
const RETRIES: u32 = 4;
/// The longest wait between tries. A provider asking for longer (a limit
/// that resets in minutes) gets its error shown instead.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// A request that didn't work: what to tell the person, and whether another
/// try may (a busy or briefly broken service), with how long the provider
/// asked us to wait if it said.
struct Failed {
    message: String,
    retry: Option<Option<Duration>>,
    /// 401 or 403: a cached sign-in token may have gone stale early.
    refused: bool,
}

impl Failed {
    fn fatal(message: impl Into<String>) -> Self {
        Failed { message: message.into(), retry: None, refused: false }
    }
    fn again(message: impl Into<String>, after: Option<Duration>) -> Self {
        Failed { message: message.into(), retry: Some(after), refused: false }
    }
}

impl From<String> for Failed {
    fn from(message: String) -> Self {
        Failed::fatal(message)
    }
}

/// An error status worth another try: busy, rate-limited or briefly broken.
/// Not a 429 that means the account is out of credit: waiting won't fix that.
fn worth_retrying(code: u16, detail: &str) -> bool {
    let d = detail.to_lowercase();
    let out_of_credit = ["insufficient_quota", "billing", "credit balance", "spending limit", "exceeded your current quota", "payment"].iter().any(|w| d.contains(w));
    matches!(code, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529) && !out_of_credit
}

/// How long the provider asked us to wait, from `retry-after-ms` or
/// `Retry-After` (seconds or an HTTP date).
fn retry_after(h: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(ms) = h.get("retry-after-ms").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse::<f64>().ok()) {
        return Some(Duration::from_millis(ms.max(0.0) as u64));
    }
    let s = h.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = s.parse::<f64>() {
        return Some(Duration::from_millis((secs.max(0.0) * 1000.0) as u64));
    }
    let when = chrono::DateTime::parse_from_rfc2822(s).ok()?;
    (when.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().ok()
}

/// The wait before try number `tries + 1`: what the provider asked for, or
/// 1, 2, 4, 8, 16 s give or take a quarter (so many chats don't all come
/// back at once), never over 30 s.
fn backoff(tries: u32, asked: Option<Duration>) -> Duration {
    if let Some(a) = asked {
        return a;
    }
    let base = 1000u64 << tries.min(4);
    let spread = base / 4;
    let jitter = (uuid::Uuid::new_v4().as_u128() % (2 * spread as u128 + 1)) as u64;
    Duration::from_millis(base - spread + jitter).min(Duration::from_secs(30))
}

/// After a failed try: how long to wait before the next, or None to give up
/// (not worth it, out of tries, the provider wants too long, or the person
/// has already seen part of the reply, which a new try would repeat).
fn next_wait(f: &Failed, tries: u32, began: bool) -> Option<Duration> {
    let asked = f.retry?;
    if began || tries >= RETRIES {
        return None;
    }
    Some(backoff(tries, asked)).filter(|w| *w <= LONGEST_WAIT)
}

/// Waits, checking Stop four times a second. False when stopped.
async fn pause(wait: Duration, cancel: &AtomicBool) -> bool {
    let end = std::time::Instant::now() + wait;
    while std::time::Instant::now() < end {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        tokio::time::sleep((end - std::time::Instant::now()).min(Duration::from_millis(250))).await;
    }
    !cancel.load(Ordering::Relaxed)
}

async fn failure(resp: reqwest::Response, who: &str) -> Failed {
    let status = resp.status();
    let after = retry_after(resp.headers());
    let text = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or(text);
    let message = match status.as_u16() {
        401 | 403 => format!("{who} didn't accept the API key ({status}). Check it in Settings → Cloud models; for a server you sign in to (like Open WebUI), press Sign in… there instead."),
        429 => format!("{who} says you've hit a rate or spending limit: {detail}"),
        529 => format!("{who} is overloaded right now. Try again in a minute, or use a model on this PC."),
        _ => format!("{who} returned {status}: {detail}"),
    };
    let mut f = if worth_retrying(status.as_u16(), &detail) { Failed::again(message, after) } else { Failed::fatal(message) };
    f.refused = matches!(status.as_u16(), 401 | 403);
    f
}

/// Entra and Google tokens are cached; IT can revoke one before its stated
/// expiry, so a refusal is worth one try with a fresh token.
fn fresh_token_helps(f: &Failed, kind: Kind, e: &enterprise::Ent, began: bool) -> bool {
    f.refused && !began && matches!((kind, e.auth), (Kind::Azure, enterprise::Auth::Entra) | (Kind::Vertex, _))
}

/// A failed send: one that never connected, or whose request didn't go
/// out, is worth another try; a timeout isn't (900 s means it's stuck).
fn send_failed(e: reqwest::Error, who: &str) -> Failed {
    let m = format!("Couldn't reach {who}: {e}");
    if e.is_connect() || (e.is_request() && !e.is_timeout()) {
        Failed::again(m, None)
    } else {
        Failed::fatal(m)
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
    Finished::default()
}

async fn anthropic_stream(client: &reqwest::Client, t: &Target, extra: &Value, messages: &[Value], tools: Option<&Value>, cancel: &AtomicBool, on_delta: &mut impl FnMut(Delta)) -> Result<Finished, Failed> {
    let (mut system, mut msgs, mut tools) = to_anthropic(messages, tools);
    let id = t.model.id.as_str();
    // A reply shape a program asked for in OpenAI's form: a schema becomes
    // structured output; plain JSON is asked for in words.
    let format = extra.get("response_format");
    let schema = format.filter(|f| f.get("type").and_then(Value::as_str) == Some("json_schema")).and_then(|f| f.pointer("/json_schema/schema")).cloned();
    if schema.is_none() && format.and_then(|f| f.get("type")).and_then(Value::as_str) == Some("json_object") {
        system = format!("{system}\n\nReply with one JSON object and nothing else.").trim().to_string();
    }
    let system = mark_cache(&system, &mut msgs, &mut tools);
    let mut body = json!({
        "model": id,
        "max_tokens": t.model.max_output.clamp(1024, 64_000),
        "messages": msgs,
        "stream": true,
    });
    if !system.is_null() {
        body["system"] = system;
    }
    if let Some(schema) = schema {
        body["output_config"] = json!({ "format": { "type": "json_schema", "schema": schema } });
    }
    if let Some(n) = extra.get("max_tokens").and_then(Value::as_u64) {
        body["max_tokens"] = json!(n.clamp(1, 128_000));
    }
    match extra.get("stop") {
        Some(Value::String(s)) => body["stop_sequences"] = json!([s]),
        Some(Value::Array(a)) => body["stop_sequences"] = json!(a),
        _ => {}
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    let mut req = if t.kind == Kind::Vertex {
        // Vertex names the model in the address and its own API version in the body.
        body.as_object_mut().map(|o| o.remove("model"));
        body["anthropic_version"] = json!("vertex-2023-10-16");
        let token = enterprise::bearer(client, t).await?;
        client.post(enterprise::vertex_claude_url(t)).bearer_auth(token)
    } else {
        client.post(format!("{}/v1/messages", t.base_url)).header("x-api-key", &t.key).header("anthropic-version", ANTHROPIC_VERSION)
    };
    req = t.wire.add(req);
    if adaptive(id) {
        // Readable summaries of its reasoning for the thinking panel.
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
        // Advanced mode's thinking switch maps to effort (current Claude
        // models always think; temperature and top-p aren't accepted).
        match extra.pointer("/chat_template_kwargs/enable_thinking").and_then(Value::as_bool) {
            Some(false) => body["output_config"]["effort"] = json!("low"),
            Some(true) => body["output_config"]["effort"] = json!("high"),
            None => {}
        }
    }
    if wants_fallback(id) && t.kind == Kind::Anthropic {
        body["fallbacks"] = json!("default");
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = req.json(&body).timeout(Duration::from_secs(900)).send().await.map_err(|e| send_failed(e, &t.provider))?;
    if !resp.status().is_success() {
        return Err(failure(resp, &t.provider).await);
    }
    let mut out = empty();
    // Blocks by index, to save for replay (thinking keeps its signature).
    let mut blocks: Vec<Value> = Vec::new();
    let mut json_parts: HashMap<usize, String> = HashMap::new();
    let mut stop = String::new();
    let mut refusal = None;
    let mut stream_error: Option<(String, String)> = None;
    out.cancelled = sse(resp, cancel, |v| {
        match v["type"].as_str() {
            Some("message_start") => {
                let u = &v["message"]["usage"];
                let input = u["input_tokens"].as_u64().unwrap_or(0) + u["cache_creation_input_tokens"].as_u64().unwrap_or(0) + u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                out.prompt_tokens = Some(input as u32);
                out.cache_read = u["cache_read_input_tokens"].as_u64().map(|n| n as u32);
                out.cache_write = u["cache_creation_input_tokens"].as_u64().map(|n| n as u32);
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
                let kind = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("").to_string();
                stream_error = Some((kind, v.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string()));
                return false;
            }
            _ => {}
        }
        true
    })
    .await
    // A reply that broke off may be tried again (`next_wait` won't if any was shown).
    .map_err(|m| Failed::again(m, None))?;
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
    if let Some((kind, m)) = stream_error {
        let msg = format!("{} stopped with an error: {m}", t.provider);
        return Err(if matches!(kind.as_str(), "overloaded_error" | "api_error" | "rate_limit_error") { Failed::again(msg, None) } else { Failed::fatal(msg) });
    }
    if stop == "refusal" && out.content.trim().is_empty() && out.tool_calls.is_empty() {
        let why = refusal.map(|c| format!(" ({c})")).unwrap_or_default();
        return Err(Failed::fatal(format!("{} declined to answer this{why}. Try rephrasing, or use a model on this PC.", t.provider)));
    }
    out.raw = Some(Value::Array(blocks));
    Ok(out)
}

async fn openai_stream(client: &reqwest::Client, t: &Target, extra: &Value, messages: &[Value], tools: Option<&Value>, cancel: &AtomicBool, on_delta: &mut impl FnMut(Delta)) -> Result<Finished, Failed> {
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
    let mut body = json!({ "model": enterprise::openai_model(t), "messages": msgs, "stream": true, "stream_options": { "include_usage": true } });
    if let Some(tl) = tools.filter(|t| t.as_array().is_some_and(|a| !a.is_empty())) {
        body["tools"] = tl.clone();
    }
    // Only the standard sampling settings (local-engine ones are left out),
    // and a reply shape a program asked for through the local API.
    for k in ["temperature", "top_p", "presence_penalty", "frequency_penalty", "seed", "response_format", "stop", "max_tokens"] {
        if let Some(v) = extra.get(k) {
            body[k] = v.clone();
        }
    }
    // OpenAI's own service caches by itself; a key per chat keeps its steps on
    // one cache. Other services may refuse a field they don't know.
    if t.base_url.contains("api.openai.com") {
        if let Some(chat) = t.chat.lock().unwrap().clone() {
            body["prompt_cache_key"] = json!(chat);
        }
    }
    let mut req = t.wire.add(enterprise::openai_request(client, t).await?);
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
        .map_err(|e| send_failed(e, &t.provider))?;
    if !resp.status().is_success() {
        return Err(failure(resp, &t.provider).await);
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
            out.cache_read = u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64).map(|n| n as u32);
        }
        true
    })
    .await
    .map_err(|m| Failed::again(m, None))?;
    Ok(out)
}

// ---------- listing a provider's models ----------

async fn list_models(state: Option<&AppState>, p: &Provider, key: &str, cookies: &str, client: &reqwest::Client, w: &Wire) -> Result<Vec<CloudModel>, String> {
    let base = p.base_url.trim_end_matches('/');
    let mut out = Vec::new();
    match p.kind {
        Kind::Azure | Kind::Bedrock | Kind::Vertex => return enterprise::list_models(state, p, key, client, w).await,
        Kind::Anthropic => {
            let resp = w.add(client.get(format!("{base}/v1/models?limit=100")).header("x-api-key", key).header("anthropic-version", ANTHROPIC_VERSION)).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(failure(resp, &p.name).await.message);
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
            let mut req = w.add(client.get(format!("{base}/models")).bearer_auth(key));
            if !cookies.is_empty() {
                req = req.header("cookie", cookies);
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let to_login = resp.status().is_redirection() || resp.url().host_str().is_some_and(|h| h.contains("login.") || h.contains("microsoftonline"));
            if to_login {
                return Err(not_an_api(&p.name, true));
            }
            if !resp.status().is_success() {
                return Err(failure(resp, &p.name).await.message);
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

pub mod bedrock;
pub mod cmds;
pub mod enterprise;
pub mod ent_cmds;
pub mod routing;
pub mod usage;
pub use cmds::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ent_tests;

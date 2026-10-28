// SPDX-License-Identifier: AGPL-3.0-only
//! The built-in browser: its own window with its own profile (cookies and
//! sign-ins stay apart from the app and from the user's other browsers).
//!
//! The assistant drives it through WebView2's in-process DevTools channel
//! (`CallDevToolsProtocolMethod`), so there is no debugging port for other
//! programs to find. Pages in it get none of the app's permissions: the
//! window's label isn't in any capability.
//!
//! Safety:
//! - Only http and https; never this PC or the home network.
//! - Downloads are off; links that open new windows open here instead.
//! - Submitting a form, buying, sending, posting or deleting always asks the
//!   user first, in every mode.
//! - The assistant never types passwords or payment details; the user types
//!   those in the window.
//! - Going Offline closes it.

use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const LABEL: &str = "browser";
const TITLE: &str = "SulcusAI browser";

/// Page text sent to the model, in characters.
pub const MAX_TEXT: usize = 10_000;

/// Whether the window may go to this address (checked on every navigation).
pub fn navigable(url: &tauri::Url) -> bool {
    navigable_with(url, allow_local(), lookup)
}

/// A name's addresses (None when it can't be found within 2 seconds).
/// This runs on the window's own thread, so answers are kept for a few
/// minutes: the same site isn't looked up on every page.
fn lookup(host: &str, port: u16) -> Option<Vec<IpAddr>> {
    use std::net::ToSocketAddrs;
    type Seen = std::collections::HashMap<String, (std::time::Instant, Vec<IpAddr>)>;
    static SEEN: std::sync::LazyLock<std::sync::Mutex<Seen>> = std::sync::LazyLock::new(Default::default);
    if let Some((at, addrs)) = SEEN.lock().unwrap().get(host) {
        if at.elapsed() < Duration::from_secs(300) {
            return Some(addrs.clone());
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let target = (host.to_string(), port);
    std::thread::spawn(move || {
        let _ = tx.send(target.to_socket_addrs().map(|a| a.map(|s| s.ip()).collect::<Vec<_>>()));
    });
    let addrs = rx.recv_timeout(Duration::from_secs(2)).ok()?.ok()?;
    let mut seen = SEEN.lock().unwrap();
    if seen.len() > 500 {
        seen.clear();
    }
    seen.insert(host.to_string(), (std::time::Instant::now(), addrs.clone()));
    Some(addrs)
}

fn navigable_with(url: &tauri::Url, allow_local: bool, lookup: impl Fn(&str, u16) -> Option<Vec<IpAddr>>) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        // about:blank is how a fresh window starts.
        return url.as_str() == "about:blank";
    }
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_matches(['[', ']']).trim_end_matches('.').to_ascii_lowercase();
    // A server on this PC the person chose to view, or the assistant started.
    if crate::ports::local_port(url.as_str()).is_some_and(crate::ports::previewable) {
        return true;
    }
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal") || !host.contains('.') && host.parse::<IpAddr>().is_err() {
        return allow_local;
    }
    if allow_local {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => crate::web::is_public(ip),
        // A name must point only at public addresses: some names lead into
        // the home network (192-168-1-1.nip.io is the router).
        Err(_) => lookup(&host, url.port_or_known_default().unwrap_or(443)).is_some_and(|addrs| !addrs.is_empty() && addrs.iter().all(|ip| crate::web::is_public(*ip))),
    }
}

#[cfg(not(test))]
fn allow_local() -> bool {
    false
}

/// Tests serve their pages from 127.0.0.1.
#[cfg(test)]
fn allow_local() -> bool {
    true
}

/// The browser follows the connectivity level (or this chat's globe); the
/// Web search feature alone doesn't open it.
pub fn allowed_for_chat(conn: &rusqlite::Connection, chat_id: &str) -> bool {
    let level = crate::db::settings(conn).connectivity;
    let chat_web: bool = conn.query_row("SELECT web FROM chats WHERE id = ?1", [chat_id], |r| r.get::<_, i64>(0)).map(|v| v != 0).unwrap_or(false);
    crate::net::allowed(level, crate::net::Purpose::Web, chat_web)
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// The browser window, opened (and shown) if it isn't already.
pub fn open(app: &AppHandle, visible: bool) -> Result<WebviewWindow, String> {
    crate::offline::check(crate::offline::Keep::Web)?;
    if let Some(w) = window(app) {
        if visible {
            let _ = w.show();
            let _ = w.unminimize();
        }
        return Ok(w);
    }
    let profile = app.state::<std::sync::Arc<crate::AppState>>().paths.data.join("browser");
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let opener = app.clone();
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External("about:blank".parse().unwrap()))
        .title(TITLE)
        .inner_size(1180.0, 820.0)
        .visible(visible)
        .data_directory(profile)
        .on_navigation(navigable)
        // Pop-ups and target=_blank links open in this window instead.
        .on_new_window(move |url, _| {
            if navigable(&url) {
                if let Some(w) = window(&opener) {
                    let _ = w.navigate(url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .on_download(|_, _| false)
        .build()
        .map_err(|e| format!("Couldn't open the browser: {e}"))
}

/// Closes the browser window, the search window and every web viewer (going
/// offline).
pub fn close(app: &AppHandle) {
    for label in [LABEL, SEARCH_LABEL] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.close();
        }
    }
    crate::viewer::close_all(app);
}

// ---------- the search window ----------

const SEARCH_LABEL: &str = "search";
/// One search at a time in it.
static SEARCHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static SEARCH_USED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// DuckDuckGo's results as its own page shows them, once they're there (or
/// none after 10 seconds).
const RESULTS_JS: &str = r#"new Promise((done) => {
  const read = () => [...document.querySelectorAll('article[data-testid="result"]')].map((r) => {
    const a = r.querySelector('a[data-testid="result-title-a"]');
    const s = r.querySelector('[data-result="snippet"]');
    return a ? { title: a.innerText.trim(), url: a.href, snippet: s ? s.innerText.trim() : '' } : null;
  }).filter(Boolean);
  const started = Date.now();
  const look = () => {
    const found = read();
    if (found.length || Date.now() - started > 10000 || (document.body && document.body.innerText.includes('No results found'))) done(found);
    else setTimeout(look, 400);
  };
  look();
})"#;

/// A hidden window for searches DuckDuckGo's plain page turns away: the same
/// search on its full page, in a real browser, as a person would make it. Its
/// profile is its own (no sign-ins, nothing from the user's browsing).
fn search_window(app: &AppHandle) -> Result<Webview, String> {
    if let Some(w) = app.get_webview_window(SEARCH_LABEL) {
        return Ok(w.as_ref().clone());
    }
    let profile = app.state::<std::sync::Arc<crate::AppState>>().paths.data.join("search-browser");
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let w = WebviewWindowBuilder::new(app, SEARCH_LABEL, WebviewUrl::External("about:blank".parse().unwrap()))
        .title("SulcusAI search")
        .inner_size(1180.0, 900.0)
        .visible(false)
        .focused(false)
        .skip_taskbar(true)
        .data_directory(profile)
        .on_navigation(navigable)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .build()
        .map_err(|e| format!("Couldn't open the search window: {e}"))?;
    Ok(w.as_ref().clone())
}

/// A web search on DuckDuckGo's full page in the hidden search window. The
/// window closes after two idle minutes.
pub async fn search(app: &AppHandle, query: &str) -> Result<Vec<crate::web::Hit>, String> {
    crate::offline::check(crate::offline::Keep::Web)?;
    let _turn = SEARCHING.lock().await;
    *SEARCH_USED.lock().unwrap() = Some(std::time::Instant::now());
    let w = search_window(app)?;
    let mut url = tauri::Url::parse("https://duckduckgo.com/").unwrap();
    url.query_pairs_mut().append_pair("q", query).append_pair("kl", "us-en");
    let found = match navigate(&w, url.as_str()).await {
        Ok(()) => eval(&w, RESULTS_JS).await,
        Err(e) => Err(e),
    };
    *SEARCH_USED.lock().unwrap() = Some(std::time::Instant::now());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        let _turn = SEARCHING.lock().await;
        if SEARCH_USED.lock().unwrap().is_some_and(|t| t.elapsed() >= Duration::from_secs(115)) {
            if let Some(w) = app.get_webview_window(SEARCH_LABEL) {
                let _ = w.close();
            }
        }
    });
    let s = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    Ok(found?
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| crate::web::Hit { title: s(r, "title"), url: s(r, "url"), snippet: s(r, "snippet") })
        .filter(|h| h.url.starts_with("http") && !h.url.contains("duckduckgo.com/y.js"))
        .collect())
}

/// Calls a DevTools Protocol method on the browser's page, in process.
#[cfg(windows)]
pub async fn cdp(w: &Webview, method: &str, params: Value) -> Result<Value, String> {
    use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR};
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    let (method_s, params_s) = (method.to_string(), params.to_string());
    let send = tx.clone();
    w.with_webview(move |pw| {
        let fail = |e: String| {
            if let Some(tx) = send.lock().unwrap().take() {
                let _ = tx.send(Err(e));
            }
        };
        // SAFETY: COM calls on the webview's own thread, with strings that
        // outlive the call.
        let started = unsafe {
            (|| {
                let core = pw.controller().CoreWebView2()?;
                let m = CoTaskMemPWSTR::from(method_s.as_str());
                let p = CoTaskMemPWSTR::from(params_s.as_str());
                let done = send.clone();
                let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, json| {
                    if let Some(tx) = done.lock().unwrap().take() {
                        let _ = tx.send(hr.map(|_| json).map_err(|e| e.message().to_string()));
                    }
                    Ok(())
                }));
                core.CallDevToolsProtocolMethod(*m.as_ref().as_pcwstr(), *p.as_ref().as_pcwstr(), &handler)
            })()
        };
        if let Err(e) = started {
            fail(e.message().to_string());
        }
    })
    .map_err(|e| e.to_string())?;
    let json = tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .map_err(|_| "The page didn't respond.".to_string())?
        .map_err(|_| "The browser closed.".to_string())??;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub async fn cdp(_w: &Webview, _method: &str, _params: Value) -> Result<Value, String> {
    Err("The built-in browser needs Windows for now.".into())
}

/// Runs a script in the page and returns its value.
pub async fn eval(w: &Webview, js: &str) -> Result<Value, String> {
    let r = cdp(w, "Runtime.evaluate", json!({ "expression": js, "returnByValue": true, "awaitPromise": true })).await?;
    if let Some(ex) = r.get("exceptionDetails") {
        let msg = ex.pointer("/exception/description").or_else(|| ex.get("text")).and_then(Value::as_str).unwrap_or("script error");
        return Err(msg.chars().take(300).collect());
    }
    Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

/// Waits until the page has loaded (or 15 seconds have passed).
async fn settle(w: &Webview) {
    tokio::time::sleep(Duration::from_millis(500)).await;
    for _ in 0..30 {
        if let Ok(Value::String(s)) = eval(w, "document.readyState").await {
            if s == "complete" {
                tokio::time::sleep(Duration::from_millis(300)).await;
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Shows in the page that the assistant is using it.
const BANNER_JS: &str = r#"(() => {
  if (document.getElementById('__sulcus_banner')) return;
  const b = document.createElement('div');
  b.id = '__sulcus_banner';
  b.textContent = 'SulcusAI is using this page. Stop it from the chat.';
  b.style.cssText = 'position:fixed;left:50%;bottom:12px;transform:translateX(-50%);z-index:2147483647;background:#0f766e;color:#fff;font:13px system-ui,sans-serif;padding:6px 14px;border-radius:99px;box-shadow:0 2px 8px rgba(0,0,0,.3);pointer-events:none';
  (document.body || document.documentElement).appendChild(b);
})()"#;

const READ_JS: &str = r#"(() => {
  const MAX_EL = 150;
  const shown = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const inView = (el) => { const r = el.getBoundingClientRect(); return r.bottom > 0 && r.top < innerHeight; };
  const sel = 'a[href], button, input:not([type=hidden]), select, textarea, summary, [role=button], [role=link], [role=tab], [role=menuitem], [role=checkbox], [role=option], [role=searchbox], [role=textbox], [contenteditable=""], [contenteditable=true]';
  document.querySelectorAll('[data-sulcus-ref]').forEach((e) => e.removeAttribute('data-sulcus-ref'));
  const all = [...document.querySelectorAll(sel)].filter((el) => shown(el) && el.id !== '__sulcus_banner');
  const ordered = all.filter(inView).concat(all.filter((el) => !inView(el)));
  const label = (el) => (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.title || el.getAttribute('alt') || el.name || '').replace(/\s+/g, ' ').trim().slice(0, 80);
  const elements = ordered.slice(0, MAX_EL).map((el, i) => {
    const ref = i + 1;
    el.setAttribute('data-sulcus-ref', String(ref));
    const tag = el.tagName.toLowerCase();
    const o = { ref, kind: tag === 'input' ? 'input:' + (el.type || 'text') : (el.getAttribute('role') || tag), label: label(el) };
    if (tag === 'a') o.href = (el.getAttribute('href') || '').slice(0, 120);
    if (tag === 'select') o.options = [...el.options].slice(0, 12).map((x) => x.text.trim());
    if ((tag === 'input' || tag === 'textarea') && el.type !== 'password' && el.value) o.value = String(el.value).slice(0, 80);
    return o;
  });
  const banner = document.getElementById('__sulcus_banner');
  const text = (document.body ? document.body.innerText : '').replace(banner ? banner.textContent : '\u0000', '').replace(/\n{3,}/g, '\n\n').trim();
  return { url: location.href, title: document.title, text, elements, more: all.length > MAX_EL };
})()"#;

/// Scrolls an element into view and describes it (for the safety checks).
fn target_js(r: u32) -> String {
    format!(
        r#"(() => {{
  const el = document.querySelector('[data-sulcus-ref="{r}"]');
  if (!el) return null;
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  const b = el.getBoundingClientRect();
  const tag = el.tagName.toLowerCase();
  const type = (el.getAttribute('type') || '').toLowerCase();
  const form = el.form || el.closest('form');
  return {{
    x: b.left + b.width / 2, y: b.top + b.height / 2, tag, type,
    label: (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.title || el.name || '').replace(/\s+/g, ' ').trim().slice(0, 80),
    href: tag === 'a' ? el.href : null,
    form: form ? {{ method: (form.getAttribute('method') || 'get').toLowerCase(), action: form.action || location.href, search: form.getAttribute('role') === 'search' || !!form.querySelector('input[type=search]') }} : null,
    autocomplete: (el.getAttribute('autocomplete') || '').toLowerCase(),
    name: ((el.name || '') + ' ' + (el.id || '')).toLowerCase(),
    host: location.host,
  }};
}})()"#
    )
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct FormInfo {
    pub method: String,
    pub action: String,
    pub search: bool,
}

/// An element the assistant wants to use.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Target {
    pub x: f64,
    pub y: f64,
    pub tag: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    pub href: Option<String>,
    pub form: Option<FormInfo>,
    pub autocomplete: String,
    pub name: String,
    pub host: String,
}

pub async fn target(w: &Webview, r: u32) -> Result<Target, String> {
    match eval(w, &target_js(r)).await? {
        Value::Null => Err(format!("There's no element [{r}] on the page now. Read the page again to get fresh numbers.")),
        v => serde_json::from_value(v).map_err(|e| e.to_string()),
    }
}

/// Fields the assistant never fills in: the user types these themselves.
pub fn private_field(t: &Target) -> bool {
    let ac = t.autocomplete.as_str();
    t.kind == "password"
        || ac.starts_with("cc-")
        || matches!(ac, "current-password" | "new-password" | "one-time-code")
        || regex::Regex::new(r"(passw|card.?num|ccnum|cvv|cvc|csc|iban|routing|ssn|social.?sec|account.?num)").unwrap().is_match(&t.name)
}

const COMMITTING: &str = r"(?i)\b(buy|purchase|order|checkout|check out|pay|payment|subscribe|donate|send|post|publish|submit|delete|remove|confirm|sign up|register|transfer|book|reserve|apply|unsubscribe|place)\b";

/// Why this needs the user's go-ahead first, if it does: it submits a form
/// (other than a search), or it reads like buying, sending or deleting.
pub fn needs_confirmation(t: &Target, submitting: bool) -> Option<String> {
    let commit_words = regex::Regex::new(COMMITTING).unwrap();
    if commit_words.is_match(&t.label) {
        return Some(format!("“{}” may buy, send, post or delete something", t.label));
    }
    let is_submit_button = (t.tag == "button" && matches!(t.kind.as_str(), "" | "submit")) || (t.tag == "input" && matches!(t.kind.as_str(), "submit" | "image"));
    if let Some(f) = &t.form {
        if (is_submit_button || submitting) && !(f.search || f.method == "get") {
            return Some("it submits a form".into());
        }
    }
    None
}

/// The page, as the model reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub text: String,
    pub elements: Vec<Value>,
    #[serde(default)]
    pub more: bool,
}

pub async fn read(w: &Webview) -> Result<Page, String> {
    let _ = eval(w, BANNER_JS).await;
    serde_json::from_value(eval(w, READ_JS).await?).map_err(|e| e.to_string())
}

impl Page {
    /// Elements as numbered lines, then the text.
    pub fn render(&self, max_text: usize) -> String {
        let els: Vec<String> = self
            .elements
            .iter()
            .map(|e| {
                let mut line = format!("[{}] {} “{}”", e["ref"], e["kind"].as_str().unwrap_or(""), e["label"].as_str().unwrap_or(""));
                if let Some(h) = e["href"].as_str().filter(|h| !h.is_empty()) {
                    line.push_str(&format!(" → {h}"));
                }
                if let Some(v) = e["value"].as_str() {
                    line.push_str(&format!(" (now: {v})"));
                }
                if let Some(opts) = e["options"].as_array() {
                    let o: Vec<&str> = opts.iter().filter_map(Value::as_str).collect();
                    line.push_str(&format!(" options: {}", o.join(" | ")));
                }
                line
            })
            .collect();
        let text: String = self.text.chars().take(max_text).collect();
        let cut = if self.text.chars().count() > max_text { "\n[The page goes on.]" } else { "" };
        format!(
            "Page: {}\nAddress: {}\n\nThings you can use (by number):\n{}{}\n\nText:\n{text}{cut}",
            self.title,
            self.url,
            els.join("\n"),
            if self.more { "\n(more further down)" } else { "" }
        )
    }
}

pub async fn navigate(w: &Webview, url: &str) -> Result<(), String> {
    let r = cdp(w, "Page.navigate", json!({ "url": url })).await?;
    if let Some(e) = r.get("errorText").and_then(Value::as_str) {
        return Err(format!("Couldn't open the page ({e})."));
    }
    settle(w).await;
    Ok(())
}

pub async fn click(w: &Webview, t: &Target) -> Result<(), String> {
    for (kind, extra) in [("mouseMoved", json!({})), ("mousePressed", json!({ "button": "left", "clickCount": 1 })), ("mouseReleased", json!({ "button": "left", "clickCount": 1 }))] {
        let mut p = json!({ "type": kind, "x": t.x, "y": t.y });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        cdp(w, "Input.dispatchMouseEvent", p).await?;
    }
    settle(w).await;
    Ok(())
}

pub async fn type_into(w: &Webview, r: u32, t: &Target, text: &str, submit: bool) -> Result<(), String> {
    if t.tag == "select" {
        let js = format!(
            r#"(() => {{ const el = document.querySelector('[data-sulcus-ref="{r}"]'); const want = {}; const o = [...el.options].find((x) => x.text.trim().toLowerCase() === want.toLowerCase() || x.value === want); if (!o) return false; el.value = o.value; el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.dispatchEvent(new Event('change', {{ bubbles: true }})); return true; }})()"#,
            serde_json::to_string(text).unwrap()
        );
        return match eval(w, &js).await? {
            Value::Bool(true) => Ok(()),
            _ => Err(format!("“{text}” isn't one of the choices.")),
        };
    }
    let focus = format!(
        r#"(() => {{ const el = document.querySelector('[data-sulcus-ref="{r}"]'); el.focus(); if (el.select) el.select(); else document.execCommand('selectAll'); }})()"#
    );
    eval(w, &focus).await?;
    cdp(w, "Input.insertText", json!({ "text": text })).await?;
    if submit {
        for kind in ["rawKeyDown", "char", "keyUp"] {
            let mut p = json!({ "type": kind, "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 });
            if kind == "char" {
                p["text"] = json!("\r");
            }
            cdp(w, "Input.dispatchKeyEvent", p).await?;
        }
        settle(w).await;
    }
    Ok(())
}

pub async fn back(w: &Webview) -> Result<(), String> {
    eval(w, "history.back()").await?;
    settle(w).await;
    Ok(())
}

/// Scrolls the page (or the part of it under the middle) by screens: down
/// or up. A wheel turn, so pages that scroll inside a box scroll too.
pub async fn scroll(w: &Webview, down: bool, screens: f64) -> Result<(), String> {
    let size = eval(w, "[innerWidth, innerHeight]").await?;
    let (x, y) = (size[0].as_f64().unwrap_or(800.0) / 2.0, size[1].as_f64().unwrap_or(600.0) / 2.0);
    let dy = size[1].as_f64().unwrap_or(600.0) * 0.85 * screens.clamp(0.25, 10.0) * if down { 1.0 } else { -1.0 };
    cdp(w, "Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0, "deltaY": dy })).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    Ok(())
}

/// The keys the assistant may press (Enter goes with typing, so a form is
/// checked before it's sent): name, code and Windows key code.
pub const KEYS: [(&str, &str, u32); 13] = [
    ("Escape", "Escape", 27),
    ("Tab", "Tab", 9),
    ("ArrowDown", "ArrowDown", 40),
    ("ArrowUp", "ArrowUp", 38),
    ("ArrowLeft", "ArrowLeft", 37),
    ("ArrowRight", "ArrowRight", 39),
    ("PageDown", "PageDown", 34),
    ("PageUp", "PageUp", 33),
    ("Home", "Home", 36),
    ("End", "End", 35),
    ("Space", "Space", 32),
    ("Backspace", "Backspace", 8),
    ("Delete", "Delete", 46),
];

/// Presses a key in the page, as the keyboard would.
pub async fn key(w: &Webview, name: &str) -> Result<(), String> {
    let (key, code, vk) = KEYS.iter().find(|k| k.0.eq_ignore_ascii_case(name)).copied().ok_or_else(|| format!("{name} isn't a key I can press here."))?;
    let key = if key == "Space" { " " } else { key };
    for kind in ["rawKeyDown", "keyUp"] {
        cdp(w, "Input.dispatchKeyEvent", json!({ "type": kind, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk })).await?;
    }
    settle(w).await;
    Ok(())
}

// ---------- commands ----------

/// Opens the browser for the user (Browser in the sidebar).
#[tauri::command]
pub async fn open_browser(app: AppHandle, state: crate::AppStateRef<'_>) -> Result<(), String> {
    if !crate::features::is_on(&state.db.lock().unwrap(), crate::features::Feature::Browser) {
        return Err("Turn on the Browser in Features first.".into());
    }
    let level = state.settings().connectivity;
    if !crate::net::allowed(level, crate::net::Purpose::Web, false) {
        return Err("The browser needs Local AI + Web. Switch to it from the status bar.".into());
    }
    let fresh = window(&app).is_none();
    let w = open(&app, true)?;
    if fresh {
        let home = crate::browsers::search(&state.db.lock().unwrap()).home();
        w.navigate(home.parse().unwrap()).map_err(|e| e.to_string())?;
    }
    let _ = w.set_focus();
    state.log("network", "Opened the browser");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(tag: &str, kind: &str, label: &str, form: Option<(&str, bool)>) -> Target {
        Target {
            tag: tag.into(),
            kind: kind.into(),
            label: label.into(),
            form: form.map(|(m, s)| FormInfo { method: m.into(), action: String::new(), search: s }),
            ..Default::default()
        }
    }

    #[test]
    fn only_the_open_web() {
        // Names looked up as a resolver would answer them.
        let dns = |host: &str, _port: u16| -> Option<Vec<IpAddr>> {
            match host {
                "example.com" => Some(vec!["93.184.215.14".parse().unwrap()]),
                "192-168-1-1.nip.io" => Some(vec!["192.168.1.1".parse().unwrap()]),
                "mixed.example" => Some(vec!["93.184.215.14".parse().unwrap(), "10.0.0.1".parse().unwrap()]),
                "empty.example" => Some(vec![]),
                _ => None,
            }
        };
        let ok = |u: &str| navigable_with(&tauri::Url::parse(u).unwrap(), false, dns);
        assert!(ok("https://example.com/path"));
        assert!(!ok("http://192.168.1.1/"));
        assert!(!ok("http://router/"));
        assert!(!ok("http://localhost:1430/"));
        assert!(!ok("http://localhost.:1430/"));
        assert!(!ok("http://[::1]/"));
        // A name that leads into the home network, or partly does, or can't be found.
        assert!(!ok("http://192-168-1-1.nip.io/"));
        assert!(!ok("https://mixed.example/"));
        assert!(!ok("https://empty.example/"));
        assert!(!ok("https://nowhere.example/"));
        assert!(!navigable(&tauri::Url::parse("file:///C:/Windows/win.ini").unwrap()));
        assert!(!navigable(&tauri::Url::parse("javascript:alert(1)").unwrap()));
        assert!(navigable(&tauri::Url::parse("about:blank").unwrap()));
    }

    #[test]
    fn submitting_and_buying_need_the_user() {
        assert!(needs_confirmation(&t("a", "", "Pricing", None), false).is_none(), "an ordinary link");
        assert!(needs_confirmation(&t("button", "submit", "Search", Some(("get", true))), false).is_none(), "a search form");
        assert!(needs_confirmation(&t("input", "text", "", Some(("get", false))), true).is_none(), "Enter in a GET form");
        assert!(needs_confirmation(&t("button", "", "Continue", Some(("post", false))), false).is_some(), "submits a POST form");
        assert!(needs_confirmation(&t("input", "email", "", Some(("post", false))), true).is_some(), "Enter submits a POST form");
        assert!(needs_confirmation(&t("button", "button", "Place order", None), false).is_some());
        assert!(needs_confirmation(&t("a", "", "Delete account", None), false).is_some());
        assert!(needs_confirmation(&t("button", "button", "Buy now", None), false).is_some());
    }

    #[test]
    fn passwords_and_cards_are_for_the_user() {
        let mut f = t("input", "password", "", None);
        assert!(private_field(&f));
        f.kind = "text".into();
        f.autocomplete = "cc-number".into();
        assert!(private_field(&f));
        f.autocomplete = String::new();
        f.name = "cardnumber card".into();
        assert!(private_field(&f));
        f.name = "q search".into();
        assert!(!private_field(&f));
    }
}

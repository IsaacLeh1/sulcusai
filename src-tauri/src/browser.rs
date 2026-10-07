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
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const LABEL: &str = "browser";
const TITLE: &str = "SulcusAI browser";
const HOME: &str = "https://duckduckgo.com/";

/// Page text sent to the model, in characters.
pub const MAX_TEXT: usize = 10_000;

/// Whether the window may go to this address (checked on every navigation).
pub fn navigable(url: &tauri::Url) -> bool {
    navigable_with(url, allow_local())
}

fn navigable_with(url: &tauri::Url, allow_local: bool) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        // about:blank is how a fresh window starts.
        return url.as_str() == "about:blank";
    }
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || !host.contains('.') && host.parse::<IpAddr>().is_err() {
        return allow_local;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => crate::web::is_public(ip) || allow_local,
        Err(_) => true,
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

pub fn close(app: &AppHandle) {
    if let Some(w) = window(app) {
        let _ = w.close();
    }
}

/// Calls a DevTools Protocol method on the browser's page, in process.
#[cfg(windows)]
pub async fn cdp(w: &WebviewWindow, method: &str, params: Value) -> Result<Value, String> {
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
pub async fn cdp(_w: &WebviewWindow, _method: &str, _params: Value) -> Result<Value, String> {
    Err("The built-in browser needs Windows for now.".into())
}

/// Runs a script in the page and returns its value.
pub async fn eval(w: &WebviewWindow, js: &str) -> Result<Value, String> {
    let r = cdp(w, "Runtime.evaluate", json!({ "expression": js, "returnByValue": true, "awaitPromise": true })).await?;
    if let Some(ex) = r.get("exceptionDetails") {
        let msg = ex.pointer("/exception/description").or_else(|| ex.get("text")).and_then(Value::as_str).unwrap_or("script error");
        return Err(msg.chars().take(300).collect());
    }
    Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

/// Waits until the page has loaded (or 15 seconds have passed).
async fn settle(w: &WebviewWindow) {
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

pub async fn target(w: &WebviewWindow, r: u32) -> Result<Target, String> {
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

pub async fn read(w: &WebviewWindow) -> Result<Page, String> {
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

pub async fn navigate(w: &WebviewWindow, url: &str) -> Result<(), String> {
    let r = cdp(w, "Page.navigate", json!({ "url": url })).await?;
    if let Some(e) = r.get("errorText").and_then(Value::as_str) {
        return Err(format!("Couldn't open the page ({e})."));
    }
    settle(w).await;
    Ok(())
}

pub async fn click(w: &WebviewWindow, t: &Target) -> Result<(), String> {
    for (kind, extra) in [("mouseMoved", json!({})), ("mousePressed", json!({ "button": "left", "clickCount": 1 })), ("mouseReleased", json!({ "button": "left", "clickCount": 1 }))] {
        let mut p = json!({ "type": kind, "x": t.x, "y": t.y });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        cdp(w, "Input.dispatchMouseEvent", p).await?;
    }
    settle(w).await;
    Ok(())
}

pub async fn type_into(w: &WebviewWindow, r: u32, t: &Target, text: &str, submit: bool) -> Result<(), String> {
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

pub async fn back(w: &WebviewWindow) -> Result<(), String> {
    eval(w, "history.back()").await?;
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
        w.navigate(HOME.parse().unwrap()).map_err(|e| e.to_string())?;
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
        let ok = |u: &str| navigable_with(&tauri::Url::parse(u).unwrap(), false);
        assert!(ok("https://example.com/path"));
        assert!(!ok("http://192.168.1.1/"));
        assert!(!ok("http://router/"));
        assert!(!ok("http://localhost:1430/"));
        assert!(!ok("http://[::1]/"));
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

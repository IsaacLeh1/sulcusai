// SPDX-License-Identifier: AGPL-3.0-only
//! Site previews: a website the assistant built in a shared folder, served
//! on this PC only, shown in the chat's Preview panel (and openable in a
//! browser), and checked in a hidden window for script errors, missing files
//! and buttons that do nothing, so the model can fix what doesn't work.
//!
//! Each site gets its own port on 127.0.0.1, so its pages can use paths from
//! the site's root ("/style.css") and keep their storage apart from other
//! sites. Requests must name this PC as the host (no DNS rebinding), and
//! paths can't leave the site's folder.
//!
//! A site follows its chat's web setting: while the chat is offline, the
//! pages are served with a policy that blocks anything outside the site
//! (live data, scripts from a CDN), and the check says what was blocked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

use crate::AppStateRef;

/// What a page did while it was checked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Report {
    /// "error", "console", "missing", "blocked", "request_failed" or "summary".
    pub kind: String,
    #[serde(default)]
    pub message: String,
    /// For "summary": the page's title, how much text it shows, and its controls.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: usize,
    #[serde(default)]
    pub buttons: usize,
    /// Fields to type in or choose from.
    #[serde(default)]
    pub inputs: usize,
    /// Element ids the scripts looked for that the page doesn't have, and the ids it does have.
    #[serde(default)]
    pub missing_ids: Vec<String>,
    #[serde(default)]
    pub ids: Vec<String>,
    /// Buttons soon after loading (the summary is taken a few seconds later).
    #[serde(default)]
    pub early_buttons: usize,
    /// Buttons after whose click nothing on the page changed.
    #[serde(default)]
    pub quiet: Vec<String>,
    /// Whether the page's text changed by itself in the seconds after it loaded.
    #[serde(default)]
    pub changes: Option<bool>,
    #[serde(default)]
    pub dead: Vec<String>,
}

struct Site {
    chat_id: String,
    root: PathBuf,
    start: String,
    port: u16,
    web: bool,
    reports: Arc<Mutex<Vec<Report>>>,
    stop: Option<oneshot::Sender<()>>,
}

impl Site {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, self.start)
    }
}

static SITES: LazyLock<Mutex<HashMap<String, Site>>> = LazyLock::new(Default::default);

/// A site being served: its id (for the panel's buttons) and address.
#[derive(Debug, Clone, Serialize)]
pub struct Served {
    pub id: String,
    pub url: String,
    pub root: String,
    pub web: bool,
}

#[derive(Clone)]
struct Ctx {
    root: PathBuf,
    port: u16,
    web: bool,
    reports: Arc<Mutex<Vec<Report>>>,
}

/// Serves a folder (the page to start on is `start`, e.g. "index.html"),
/// reusing the server already running for it in this chat.
pub async fn serve(chat_id: &str, root: &Path, start: &str, web: bool) -> Result<Served, String> {
    let root = dunce::canonicalize(root).map_err(|_| format!("{} doesn't exist.", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} isn't a folder.", root.display()));
    }
    let start = start.trim_start_matches(['/', '\\']).replace('\\', "/");
    {
        let mut sites = SITES.lock().unwrap();
        let same = sites.iter().find(|(_, s)| s.chat_id == chat_id && s.root == root).map(|(id, s)| (id.clone(), s.web == web));
        match same {
            Some((id, true)) => {
                let s = sites.get_mut(&id).unwrap();
                s.start = start.clone();
                return Ok(Served { id, url: s.url(), root: root.display().to_string(), web });
            }
            // The chat's web setting changed: serve it again with the new policy.
            Some((id, false)) => {
                if let Some(mut old) = sites.remove(&id) {
                    if let Some(stop) = old.stop.take() {
                        let _ = stop.send(());
                    }
                }
            }
            None => {}
        }
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| format!("Couldn't start the preview: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let reports = Arc::new(Mutex::new(Vec::new()));
    let ctx = Ctx { root: root.clone(), port, web, reports: reports.clone() };
    let (tx, rx) = oneshot::channel();
    let app = Router::new().route("/__sulcus/report", post(report)).fallback(page).with_state(ctx);
    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, app).with_graceful_shutdown(async { rx.await.ok(); }).await;
    });
    // SulcusAI's own browser may show it too.
    crate::ports::allow(port);
    let id = format!("site{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let site = Site { chat_id: chat_id.to_string(), root: root.clone(), start, port, web, reports, stop: Some(tx) };
    let served = Served { id: id.clone(), url: site.url(), root: root.display().to_string(), web };
    SITES.lock().unwrap().insert(id, site);
    Ok(served)
}

/// Stops serving a site.
pub fn stop(id: &str) -> bool {
    match SITES.lock().unwrap().remove(id) {
        Some(mut s) => {
            if let Some(stop) = s.stop.take() {
                let _ = stop.send(());
            }
            true
        }
        None => false,
    }
}

/// Stops every site a chat is serving (when the chat is deleted).
pub fn stop_chat(chat_id: &str) {
    let ids: Vec<String> = SITES.lock().unwrap().iter().filter(|(_, s)| s.chat_id == chat_id).map(|(id, _)| id.clone()).collect();
    for id in ids {
        stop(&id);
    }
}

/// Only this PC may ask, by name or address.
fn local_host(headers: &HeaderMap, port: u16) -> bool {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default().to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

async fn report(State(ctx): State<Ctx>, headers: HeaderMap, body: String) -> StatusCode {
    if !local_host(&headers, ctx.port) || body.len() > 16_000 {
        return StatusCode::FORBIDDEN;
    }
    if let Ok(mut r) = serde_json::from_str::<Report>(&body) {
        r.message = r.message.chars().take(400).collect();
        r.dead.truncate(12);
        let mut all = ctx.reports.lock().unwrap();
        if all.len() < 300 {
            all.push(r);
        }
    }
    StatusCode::NO_CONTENT
}

/// What the page may reach: everything with web on; only the site itself while the chat is offline.
fn policy(web: bool) -> Option<&'static str> {
    (!web).then_some(
        "default-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob:; connect-src 'self'; img-src 'self' data: blob:; \
         media-src 'self' data: blob:; font-src 'self' data:; frame-src 'self'; worker-src 'self' blob:",
    )
}

async fn page(State(ctx): State<Ctx>, method: Method, headers: HeaderMap, uri: Uri) -> Response {
    if !local_host(&headers, ctx.port) {
        return (StatusCode::FORBIDDEN, "Only this PC can open this preview.").into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = uri.path();
    let Some(file) = file_for(&ctx.root, path) else {
        if path != "/favicon.ico" {
            let mut all = ctx.reports.lock().unwrap();
            if all.len() < 300 {
                all.push(Report { kind: "missing".into(), message: decode(path), ..Report::default() });
            }
        }
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let Ok(bytes) = tokio::fs::read(&file).await else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let mut r = (StatusCode::OK, bytes).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, kind_of(&file).parse().unwrap());
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    if let Some(p) = policy(ctx.web) {
        h.insert(header::CONTENT_SECURITY_POLICY, p.parse().unwrap());
    }
    r
}

impl Default for Report {
    fn default() -> Self {
        Report { kind: String::new(), message: String::new(), title: String::new(), text: 0, buttons: 0, inputs: 0, missing_ids: Vec::new(), ids: Vec::new(), early_buttons: 0, quiet: Vec::new(), changes: None, dead: Vec::new() }
    }
}

/// A %-encoded path, decoded (bad escapes are kept as they are).
fn decode(path: &str) -> String {
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The file a request path names inside the site's folder (a folder's
/// index.html), or None if it doesn't exist or would leave the folder.
fn file_for(root: &Path, path: &str) -> Option<PathBuf> {
    let decoded = decode(path);
    let mut full = root.to_path_buf();
    for part in decoded.split(['/', '\\']).filter(|p| !p.is_empty()) {
        if part == "." || part == ".." || part.contains(':') {
            return None;
        }
        full.push(part);
    }
    if full.is_dir() {
        full.push("index.html");
    }
    let real = dunce::canonicalize(&full).ok()?;
    (real.starts_with(root) && real.is_file()).then_some(real)
}

/// The Content-Type for a file, by its extension.
fn kind_of(path: &Path) -> &'static str {
    match path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("txt" | "md" | "csv") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("wasm") => "application/wasm",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Runs in the checked page before its own scripts: notes errors, blocked
/// and failed requests, and (after it has loaded) which buttons have nothing
/// listening for a click. Reports go back to the site's own server.
const PROBE: &str = r##"(function () {
  if (window.__sulcusProbe) return;
  window.__sulcusProbe = true;
  var send = function (o) {
    try { fetch('/__sulcus/report', { method: 'POST', body: JSON.stringify(o), keepalive: true }); } catch (e) {}
  };
  var listened = new WeakSet();
  // Pop-ups would stop the page in a window nobody sees: they're answered here
  // and counted (a message is a response to a click).
  var dialogs = 0, errors = 0;
  window.alert = function () { dialogs++; };
  window.confirm = function () { dialogs++; return true; };
  window.prompt = function (m, d) { dialogs++; return d || '1'; };
  // Ids the scripts asked for that weren't there (checked again once it has loaded).
  var asked = {};
  var byId = Document.prototype.getElementById;
  Document.prototype.getElementById = function (id) {
    var r = byId.call(this, id);
    if (r === null && typeof id === 'string') asked[id] = 1;
    return r;
  };
  var one = Document.prototype.querySelector;
  Document.prototype.querySelector = function (sel) {
    var r = one.call(this, sel);
    if (r === null && typeof sel === 'string' && /^#[A-Za-z][\w-]*$/.test(sel)) asked[sel.slice(1)] = 1;
    return r;
  };
  var add = EventTarget.prototype.addEventListener;
  EventTarget.prototype.addEventListener = function (type, fn, opts) {
    try { if (/^(click|pointerdown|pointerup|mousedown|mouseup|touchstart|submit|keydown)$/.test(type)) listened.add(this); } catch (e) {}
    return add.call(this, type, fn, opts);
  };
  window.addEventListener('error', function (e) {
    var t = e.target;
    if (t && t !== window && (t.src || t.href)) send({ kind: 'missing', message: String(t.src || t.href) });
    else errors++, send({ kind: 'error', message: String(e.message || 'Script error') + (e.filename ? ' (' + e.filename.replace(location.origin, '') + ':' + e.lineno + ')' : '') });
  }, true);
  window.addEventListener('unhandledrejection', function (e) {
    var r = e.reason;
    errors++;
    send({ kind: 'error', message: 'Unhandled promise rejection: ' + String((r && r.message) || r) });
  });
  document.addEventListener('securitypolicyviolation', function (e) {
    send({ kind: 'blocked', message: String(e.blockedURI || e.violatedDirective) });
  });
  var ce = console.error;
  console.error = function () {
    try { send({ kind: 'console', message: [].map.call(arguments, function (a) { return a && a.message ? a.message : String(a); }).join(' ') }); } catch (e) {}
    return ce.apply(console, arguments);
  };
  var of = window.fetch;
  window.fetch = function (input) {
    var url = String((input && input.url) || input);
    var p = of.apply(this, arguments);
    if (url.indexOf('/__sulcus/') >= 0) return p;
    return p.then(function (r) {
      if (!r.ok) send({ kind: 'request_failed', message: url + ' (' + r.status + ')' });
      return r;
    }, function (err) {
      send({ kind: 'request_failed', message: url + ' (' + String((err && err.message) || err) + ')' });
      throw err;
    });
  };
  var handled = function (el) {
    for (var n = el; n; n = n.parentNode) {
      if (listened.has(n) || n.onclick || (n.getAttribute && (n.getAttribute('onclick') || n.getAttribute('onsubmit')))) return true;
      if (n.tagName === 'FORM' && (n.onsubmit || n.getAttribute('action'))) return true;
    }
    return listened.has(window) || listened.has(document);
  };
  var count = function () {
    return document.querySelectorAll('button, [role=button], input[type=button], input[type=submit]').length;
  };
  var text = function () { return document.body ? document.body.innerText : ''; };
  var label = function (el) { return (el.innerText || el.value || el.getAttribute('aria-label') || el.tagName).trim().slice(0, 40); };
  var early = 0, earlyText = '';
  // Clicks each button (number fields filled in first) and notes the ones
  // after which nothing on the page changed: no new text, no message, no error.
  var tryClicks = function (els, done) {
    var quiet = [];
    document.addEventListener('submit', function (e) { e.preventDefault(); }, true);
    // Empty fields filled in the way a person would, so a form's button has
    // something to work with (a blank form rightly does nothing).
    var today = new Date().toISOString().slice(0, 10);
    [].forEach.call(document.querySelectorAll('input, select, textarea'), function (f) {
      var t = (f.type || '').toLowerCase();
      if (f.disabled || f.readOnly || ['button', 'submit', 'reset', 'hidden', 'checkbox', 'radio', 'file', 'range', 'color'].indexOf(t) >= 0) return;
      if (f.tagName === 'SELECT') {
        if (!f.value || f.selectedIndex <= 0) {
          for (var k = 0; k < f.options.length; k++) if (f.options[k].value) { f.selectedIndex = k; break; }
        }
      } else if (!f.value) {
        f.value = t === 'number' ? '25' : t === 'date' ? today : t === 'email' ? 'test@example.com' : t === 'time' ? '12:00' : t === 'month' ? today.slice(0, 7) : t === 'url' ? 'https://example.com' : t === 'tel' ? '5551234' : 'Test';
      }
      f.dispatchEvent(new Event('input', { bubbles: true }));
      f.dispatchEvent(new Event('change', { bubbles: true }));
    });
    var i = 0;
    var next = function () {
      if (i >= els.length) return done(quiet);
      var el = els[i++];
      var before = text(), d = dialogs, e = errors;
      try { el.click(); } catch (x) {}
      setTimeout(function () {
        if (text() === before && dialogs === d && errors === e) quiet.push(label(el));
        next();
      }, 150);
    };
    next();
  };
  window.addEventListener('load', function () {
    // Once soon after loading, and again after the page has had time to update
    // itself (a live list redrawn without its buttons).
    setTimeout(function () { early = count(); earlyText = text(); }, 1500);
    setTimeout(function () {
      var changes = text() !== earlyText;
      var controls = [].slice.call(document.querySelectorAll('button, [role=button], input[type=button], input[type=submit], a[href="#"], a:not([href])'));
      var dead = [];
      // Shown by its style (layout isn't there to ask in a window that never shows).
      var shown = function (el) {
        for (var n = el; n && n.nodeType === 1; n = n.parentElement) {
          var s = getComputedStyle(n);
          if (s.display === 'none' || s.visibility === 'hidden') return false;
        }
        return true;
      };
      var live = [];
      controls.forEach(function (el) {
        if (el.disabled || !shown(el)) return;
        if (el.tagName === 'A' && el.getAttribute('href') && el.getAttribute('href') !== '#') return;
        if (!handled(el)) dead.push(label(el));
        else if (el.tagName !== 'A') live.push(el);
      });
      var body = document.body;
      var summary = { kind: 'summary', title: document.title || '', text: body ? body.innerText.trim().length : 0, buttons: controls.length, inputs: document.querySelectorAll('input:not([type=button]):not([type=submit]):not([type=hidden]), select, textarea').length, dead: dead.slice(0, 12),
        missing_ids: Object.keys(asked).filter(function (id) { return byId.call(document, id) === null; }).slice(0, 12),
        ids: [].map.call(document.querySelectorAll('[id]'), function (e) { return e.id; }).slice(0, 30),
        early_buttons: early, changes: changes };
      tryClicks(live.slice(0, 8), function (quiet) {
        summary.quiet = quiet.slice(0, 12);
        send(summary);
      });
    }, 5000);
  });
})();"##;

/// Loads a site in a hidden window and returns what its page reported, once
/// it has loaded (or after 15 s).
pub async fn check(app: &AppHandle, id: &str) -> Result<Vec<Report>, String> {
    let (url, reports) = {
        let sites = SITES.lock().unwrap();
        let s = sites.get(id).ok_or("That preview has stopped.")?;
        s.reports.lock().unwrap().clear();
        (s.url(), s.reports.clone())
    };
    let parsed: tauri::Url = url.parse().map_err(|_| "That isn't an address.".to_string())?;
    let origin = (parsed.host_str().map(str::to_string), parsed.port());
    let profile = app.state::<Arc<crate::AppState>>().paths.data.join("sitecheck");
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let label = format!("sitecheck{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let w = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parsed))
        .title("SulcusAI site check")
        .inner_size(1280.0, 860.0)
        .visible(false)
        .focused(false)
        .skip_taskbar(true)
        .data_directory(profile)
        .initialization_script(PROBE)
        // It stays on the site; links elsewhere and pop-ups go nowhere.
        .on_navigation(move |u| (u.host_str().map(str::to_string), u.port()) == origin || u.as_str() == "about:blank")
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .build()
        .map_err(|e| format!("Couldn't check the site: {e}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if reports.lock().unwrap().iter().any(|r| r.kind == "summary") {
            // A moment for anything still being sent.
            tokio::time::sleep(Duration::from_millis(300)).await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = w.destroy();
    let out = reports.lock().unwrap().clone();
    Ok(out)
}

/// An address on this PC as a path on the site ("http://127.0.0.1:5000/app.js" → "/app.js").
fn site_path(what: &str) -> String {
    match reqwest::Url::parse(what) {
        Ok(u) if crate::ports::local_port(what).is_some() => u.path().to_string(),
        _ => what.to_string(),
    }
}

/// The person asked for something that updates by itself (live prices, a clock).
pub fn wants_live(asked: &str) -> bool {
    static LIVE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\b(live|real[- ]?time|live-updating|updating|ticker|countdown|clock|timer|streaming|auto-?refresh)").unwrap());
    LIVE.is_match(asked)
}

/// The person asked for things to do on the page (buy, add, search, play…),
/// not just something to look at.
pub fn wants_actions(asked: &str) -> bool {
    static DO: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(buy|sell|trade|order|add|remove|delete|edit|create|search|filter|sort|log ?in|sign ?(up|in)|submit|book|reserve|calculate|convert|play|vote|upload|send|chat|track|toggle|check ?out|cart|quiz|game|form|calculator|to-?do|planner|tracker|manage)\b").unwrap()
    });
    DO.is_match(asked)
}

/// What the check found, in words for the model: (problems, everything).
/// `asked` is what the person asked for (to notice a page with nothing to use).
pub fn describe(reports: &[Report], web: bool, asked: &str) -> (Vec<String>, String) {
    let mut problems: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut once = |p: String, problems: &mut Vec<String>| {
        if !seen.contains(&p) && problems.len() < 15 {
            seen.push(p.clone());
            problems.push(p);
        }
    };
    for r in reports {
        match r.kind.as_str() {
            "error" => once(format!("Script error: {}", r.message), &mut problems),
            "console" => once(format!("Console error: {}", r.message), &mut problems),
            // The server and the page both report a missing file; once, by its path.
            "missing" => once(format!("Missing file (404): {}", site_path(&r.message)), &mut problems),
            "request_failed" => once(format!("Request failed: {}", r.message), &mut problems),
            "blocked" if !web => once(
                format!("Blocked {} because this chat is offline: use the site's own files, or simulated data labelled as simulated (or ask to turn the web on with request_web)", r.message),
                &mut problems,
            ),
            "blocked" => once(format!("Blocked by the page's own policy: {}", r.message), &mut problems),
            _ => {}
        }
    }
    let summary = reports.iter().rev().find(|r| r.kind == "summary");
    let text = match summary {
        None => {
            once("The page didn't finish loading within 15 seconds (an endless loop or a script that never ends?)".into(), &mut problems);
            "It didn't finish loading.".to_string()
        }
        Some(s) => {
            if s.text < 40 {
                once(format!("The page shows almost nothing ({} characters of text): its content may be built by a script that failed", s.text), &mut problems);
            }
            if !s.missing_ids.is_empty() {
                let has = if s.ids.is_empty() { "none".to_string() } else { s.ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(", ") };
                once(
                    format!(
                        "The script looks for elements the page doesn't have: {}. The page's ids are: {has}. Make the ids in the HTML and the script match",
                        s.missing_ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(", ")
                    ),
                    &mut problems,
                );
            }
            if !s.quiet.is_empty() {
                once(
                    format!(
                        "Clicking these changed nothing on the page (no new text, no message): {}. Make each one do its job and show the result, for example update the cash, the totals or a list",
                        s.quiet.iter().map(|q| format!("“{q}”")).collect::<Vec<_>>().join(", ")
                    ),
                    &mut problems,
                );
            }
            if s.changes == Some(false) && wants_live(asked) {
                once(
                    "Nothing on the page changes by itself, though the user asked for it to update live: change the numbers on a timer (setInterval) and show them"
                        .to_string(),
                    &mut problems,
                );
            }
            if s.early_buttons > s.buttons {
                once(
                    format!(
                        "Buttons disappear as the page updates itself: {} soon after it loads, {} a few seconds later (the live update redraws that part without them). Keep the controls when updating, for example update only the numbers",
                        s.early_buttons, s.buttons
                    ),
                    &mut problems,
                );
            } else if s.buttons == 0 && s.inputs == 0 && wants_actions(asked) {
                once(
                    "The page has no buttons and no fields at all, so none of what the user asked to do can be done on it. Add working controls \
                     (buttons, inputs, forms) for every action they asked for, wired up in the JavaScript"
                        .to_string(),
                    &mut problems,
                );
            }
            if !s.dead.is_empty() {
                once(
                    format!("These buttons do nothing when clicked (no click handler): {}. Make every one of them work", s.dead.iter().map(|d| format!("“{d}”")).collect::<Vec<_>>().join(", ")),
                    &mut problems,
                );
            }
            format!(
                "It loaded: “{}”, {} characters of text, {} buttons ({} with nothing behind them), {} fields.",
                if s.title.is_empty() { "(no title)" } else { &s.title },
                s.text,
                s.buttons,
                s.dead.len(),
                s.inputs
            )
        }
    };
    (problems, text)
}

// ---------- commands ----------

/// Serves a site again (after a restart, or when the panel reopens), if its
/// folder is still shared with the chat.
#[tauri::command]
pub async fn preview_open(state: AppStateRef<'_>, chat_id: String, root: String, start: Option<String>) -> Result<Served, String> {
    let cipher = state.cipher()?;
    let (folders, web) = {
        let conn = state.db.lock().unwrap();
        let chat = crate::db::chat(&conn, &cipher, &chat_id).ok_or("That chat no longer exists.")?;
        (crate::projects::chat_folders(&conn, &chat), crate::browser::allowed_for_chat(&conn, &chat_id))
    };
    let sandbox = crate::sandbox::Sandbox::new(&folders);
    let path = PathBuf::from(&root);
    if !sandbox.roots().iter().any(|r| dunce::canonicalize(&path).is_ok_and(|p| p.starts_with(r))) {
        return Err("That folder isn't shared with this chat anymore.".into());
    }
    serve(&chat_id, &path, start.as_deref().unwrap_or("index.html"), web).await
}

#[tauri::command(async)]
pub fn preview_stop(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    if stop(&id) {
        state.log("settings", "Stopped a site preview");
    }
    Ok(())
}

/// Opens a preview (or a server the assistant started) in the main browser.
#[tauri::command]
pub async fn preview_open_external(app: AppHandle, state: AppStateRef<'_>, url: String) -> Result<(), String> {
    state.cipher()?;
    let port = crate::ports::local_port(&url).ok_or("Only previews on this PC open from here.")?;
    if !crate::ports::previewable(port) {
        return Err("That preview has stopped. Ask the assistant to preview the site again.".into());
    }
    let choice = crate::browsers::choice(&state.db.lock().unwrap());
    match choice {
        crate::browsers::MainBrowser::Builtin => {
            let w = crate::browser::open(&app, true)?;
            w.navigate(url.parse().map_err(|_| "That isn't an address.".to_string())?).map_err(|e| e.to_string())?;
            let _ = w.set_focus();
        }
        crate::browsers::MainBrowser::System => crate::open_with_system(std::ffi::OsStr::new(&url))?,
        crate::browsers::MainBrowser::App { name } => {
            let b = crate::browsers::installed().into_iter().find(|b| b.name == name).ok_or_else(|| format!("{name} isn't installed anymore. Pick another main browser in Settings."))?;
            std::process::Command::new(&b.exe).arg(&url).spawn().map_err(|e| format!("Couldn't start {name}: {e}"))?;
        }
    }
    state.log("settings", &format!("Opened a site preview (port {port}) in a browser"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_site() {
        let dir = std::env::temp_dir().join(format!("sulcus-preview-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("css")).unwrap();
        std::fs::write(dir.join("index.html"), "<h1>Hi</h1>").unwrap();
        std::fs::write(dir.join("css").join("site style.css"), "h1{}").unwrap();
        let root = dunce::canonicalize(&dir).unwrap();
        assert_eq!(file_for(&root, "/"), Some(root.join("index.html")));
        assert_eq!(file_for(&root, "/css/site%20style.css"), Some(root.join("css").join("site style.css")));
        assert_eq!(file_for(&root, "/../secret.txt"), None);
        assert_eq!(file_for(&root, "/%2e%2e/secret.txt"), None);
        assert_eq!(file_for(&root, "/css/../../x"), None);
        assert_eq!(file_for(&root, "/C:/Windows/win.ini"), None);
        assert_eq!(file_for(&root, "/nope.js"), None);
        assert_eq!(kind_of(Path::new("a.JS")), "text/javascript; charset=utf-8");
        assert_eq!(decode("/a%20b%zz%"), "/a b%zz%");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_check_names_what_doesnt_work() {
        let r = |kind: &str, message: &str| Report { kind: kind.into(), message: message.into(), ..Report::default() };
        let mut summary = r("summary", "");
        summary.title = "Stock Exchange".into();
        summary.text = 900;
        summary.buttons = 4;
        summary.dead = vec!["Buy".into(), "Sell".into()];
        let (problems, text) = describe(
            &[r("error", "prices is not defined (/app.js:12)"), r("error", "prices is not defined (/app.js:12)"), r("missing", "/chart.js"), r("missing", "http://127.0.0.1:5000/chart.js"), r("blocked", "https://api.example.com/quotes"), summary.clone()],
            false,
            "Build me a stock exchange site where I can buy and sell",
        );
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems[0].contains("prices is not defined"));
        assert!(problems[1].contains("/chart.js"));
        assert!(problems[2].contains("offline"));
        assert!(problems[3].contains("“Buy”, “Sell”"));
        assert!(text.contains("Stock Exchange") && text.contains("4 buttons (2 with nothing behind them), 0 fields"));
        let (problems, text) = describe(&[], true, "");
        assert_eq!(problems.len(), 1);
        assert!(text.contains("didn't finish"));
        // Asked to buy and sell, and the page has nothing to click or type in
        // (what Qwen2.5-Coder 14B made in the live test).
        let mut bare = summary;
        bare.buttons = 0;
        bare.dead.clear();
        let (problems, _) = describe(&[bare.clone()], true, "a live stock exchange where I can buy and sell shares");
        assert!(problems.len() == 1 && problems[0].contains("no buttons and no fields"), "{problems:?}");
        // The script looks for #buy but the button is #buy-button (also from the live test).
        let mut mismatched = bare.clone();
        mismatched.buttons = 2;
        mismatched.missing_ids = vec!["buy".into(), "sell".into()];
        mismatched.ids = vec!["stock-prices".into(), "buy-button".into(), "sell-button".into()];
        let (problems, _) = describe(&[mismatched], true, "buy and sell");
        assert!(problems[0].contains("#buy, #sell") && problems[0].contains("#buy-button"), "{problems:?}");
        // Eight buttons at first, none after the prices redraw (the live test again).
        let mut vanishing = bare.clone();
        vanishing.early_buttons = 8;
        let (problems, _) = describe(&[vanishing], true, "buy and sell");
        assert!(problems.len() == 1 && problems[0].contains("8 soon after it loads, 0 a few seconds later"), "{problems:?}");
        // Buttons with handlers that do nothing, and "live" prices that never move (the live test's last site).
        let mut idle = bare.clone();
        idle.buttons = 8;
        idle.quiet = vec!["Buy".into(), "Sell".into()];
        idle.changes = Some(false);
        let (problems, _) = describe(&[idle.clone()], true, "a live stock exchange website where I can buy and sell");
        assert!(problems.len() == 2 && problems[0].contains("“Buy”, “Sell”") && problems[1].contains("setInterval"), "{problems:?}");
        assert_eq!(describe(&[idle], true, "a site where I can buy and sell").0.len(), 1, "not asked to be live");
        // Something just to look at doesn't need controls.
        assert!(describe(&[bare], true, "make me a page about my dog Rex").0.is_empty());
        assert!(wants_actions("track my portfolio") && !wants_actions("a page about the history of Rome"));
    }

    #[tokio::test]
    async fn a_site_is_served_on_this_pc_only_and_follows_the_chats_web() {
        let dir = std::env::temp_dir().join(format!("sulcus-preview-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><title>T</title><p>Hello</p>").unwrap();
        let s = serve("chat-a", &dir, "index.html", false).await.unwrap();
        assert!(s.url.starts_with("http://127.0.0.1:") && s.url.ends_with("/index.html"));
        // The same folder in the same chat is the same preview.
        assert_eq!(serve("chat-a", &dir, "index.html", false).await.unwrap().id, s.id);
        let client = reqwest::Client::new();
        let r = client.get(&s.url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("connect-src 'self'"));
        assert!(r.text().await.unwrap().contains("Hello"));
        // Another host name (DNS rebinding) is refused.
        let r = client.get(&s.url).header("host", "evil.example:80").send().await.unwrap();
        assert_eq!(r.status(), 403);
        // A missing file is noted for the check.
        let base = s.url.trim_end_matches("index.html");
        assert_eq!(client.get(format!("{base}app.js")).send().await.unwrap().status(), 404);
        let posted = client.post(format!("{base}__sulcus/report")).body(r#"{"kind":"error","message":"boom"}"#).send().await.unwrap();
        assert_eq!(posted.status(), 204);
        {
            let sites = SITES.lock().unwrap();
            let kinds: Vec<String> = sites[&s.id].reports.lock().unwrap().iter().map(|r| r.kind.clone()).collect();
            assert_eq!(kinds, vec!["missing".to_string(), "error".to_string()]);
        }
        // Web turned on for the chat: served again without the policy.
        let s2 = serve("chat-a", &dir, "index.html", true).await.unwrap();
        assert_ne!(s2.id, s.id);
        let r = client.get(&s2.url).send().await.unwrap();
        assert!(r.headers().get("content-security-policy").is_none());
        stop_chat("chat-a");
        assert!(!stop(&s2.id));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

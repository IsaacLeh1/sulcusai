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
//! While it may use the web, its pages get live data through
//! /__sulcus/fetch: SulcusAI fetches it for them (pages can't read most data
//! sources themselves) and adds any API key the user approved for that
//! source, so a key is never in the page.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
    /// Buttons whose click only showed a pop-up message.
    #[serde(default)]
    pub popups: Vec<String>,
    /// What the page's buttons and fields say.
    #[serde(default)]
    pub labels: Vec<String>,
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
    state: Arc<crate::AppState>,
    chat_id: String,
    /// For live data: made on first use (plain, and one for requests carrying a key).
    clients: Arc<Mutex<Option<(reqwest::Client, reqwest::Client)>>>,
    /// When the latest live-data requests were made (a page fetching in a tight loop is slowed).
    recent: Arc<Mutex<VecDeque<Instant>>>,
    /// Hosts this site got data from (logged once each).
    hosts: Arc<Mutex<HashSet<String>>>,
}

/// Serves a folder (the page to start on is `start`, e.g. "index.html"),
/// reusing the server already running for it in this chat.
pub async fn serve(state: &Arc<crate::AppState>, chat_id: &str, root: &Path, start: &str, web: bool) -> Result<Served, String> {
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
    let ctx = Ctx {
        root: root.clone(),
        port,
        web,
        reports: reports.clone(),
        state: state.clone(),
        chat_id: chat_id.to_string(),
        clients: Default::default(),
        recent: Default::default(),
        hosts: Default::default(),
    };
    let (tx, rx) = oneshot::channel();
    let app = Router::new().route("/__sulcus/report", post(report)).route("/__sulcus/fetch", get(fetch)).fallback(page).with_state(ctx);
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
        r.quiet.truncate(12);
        r.popups.truncate(12);
        r.labels.truncate(40);
        for l in r.labels.iter_mut() {
            *l = l.chars().take(40).collect();
        }
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
    let Ok(f) = tokio::fs::File::open(&file).await else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let size = f.metadata().await.map(|m| m.len()).ok();
    let mut r = (StatusCode::OK, file_body(f)).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, kind_of(&file).parse().unwrap());
    if let Some(n) = size {
        h.insert(header::CONTENT_LENGTH, n.into());
    }
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    // The chat's web setting as it is now: a chat switched offline after the
    // site started serving gets the offline policy from its next page on.
    let web = ctx.web && crate::web::allowed_for_chat(&ctx.state.db.lock().unwrap(), &ctx.chat_id);
    if let Some(p) = policy(web) {
        h.insert(header::CONTENT_SECURITY_POLICY, p.parse().unwrap());
    }
    r
}

/// A file sent in pieces, so a site's big video never sits whole in memory.
fn file_body(f: tokio::fs::File) -> axum::body::Body {
    use tokio::io::AsyncReadExt;
    axum::body::Body::from_stream(futures_util::stream::unfold(Some(f), |f| async move {
        let mut f = f?;
        let mut buf = vec![0u8; 64 * 1024];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(buf), Some(f)))
            }
            Err(e) => Some((Err(e), None)),
        }
    }))
}

/// Most a live-data answer may be.
const FETCH_BYTES: usize = 5 * 1024 * 1024;
/// Most live-data requests a site may make in a minute.
const FETCH_PER_MINUTE: usize = 120;

fn note(ctx: &Ctx, kind: &str, message: String) {
    let mut all = ctx.reports.lock().unwrap();
    if all.len() < 300 {
        all.push(Report { kind: kind.into(), message, ..Report::default() });
    }
}

/// An answer the page can show: {"error": "..."}.
fn refuse(status: StatusCode, why: &str) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], serde_json::json!({ "error": why }).to_string()).into_response()
}

/// The address a fetch asks for (`?url=`): all of it, whether the page
/// encoded it or not ("?url=https://a.io/q?s=1&t=2" keeps its "&t=2").
fn asked_url(uri: &Uri) -> Option<String> {
    let q = uri.query()?;
    let raw = match q.strip_prefix("url=") {
        Some(rest) => decode(rest),
        None => reqwest::Url::parse(&format!("http://x/?{q}")).ok()?.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned())?,
    };
    Some(raw.trim().to_string()).filter(|v| !v.is_empty())
}

/// A stand-in where a key should be ("YOUR_API_KEY", "<key>", "demo").
pub fn placeholder_key(value: &str) -> bool {
    static FAKE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(your|my|insert|enter|put|replace|add)[ _-]?.*(key|token)|^(api[_-]?)?(key|token)([_-]?here)?$|^<[^>]*>$|^\{[^}]*\}$|^x{6,}$|^demo$|^test$|^null$|^undefined$").unwrap()
    });
    FAKE.is_match(value.trim())
}

/// Where a request carrying a key may go: https only, so the key is never
/// sent in the open. A plain-http address goes over https instead (but not
/// one on a port of its own, where https isn't likely).
fn keyed_url(mut url: reqwest::Url) -> Option<reqwest::Url> {
    match url.scheme() {
        "https" => Some(url),
        "http" if url.port().is_none() => url.set_scheme("https").ok().map(|()| url),
        _ => None,
    }
}

/// A key that goes in the address, in place of any stand-in the page put there.
fn add_key(url: &mut reqwest::Url, k: &crate::sitekeys::SiteKey) {
    if k.param.is_empty() {
        return;
    }
    let kept: Vec<(String, String)> = url.query_pairs().filter(|(n, _)| *n != k.param).map(|(n, v)| (n.into_owned(), v.into_owned())).collect();
    url.query_pairs_mut().clear().extend_pairs(kept).append_pair(&k.param, &k.key);
}

/// GET /__sulcus/fetch?url=<address>: live data for the site's pages,
/// fetched here. Only public http(s) addresses, only while the chat may use
/// the web, and only for the site's own pages (a page elsewhere can't use it,
/// or the key). A key the user approved for that host goes along; it's never
/// in what the page or the model sees.
async fn fetch(State(ctx): State<Ctx>, method: Method, headers: HeaderMap, uri: Uri) -> Response {
    if !local_host(&headers, ctx.port) {
        return (StatusCode::FORBIDDEN, "Only this PC can open this preview.").into_response();
    }
    if headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()).is_some_and(|s| s != "same-origin" && s != "none") {
        return refuse(StatusCode::FORBIDDEN, "Only this site's own pages can fetch through it.");
    }
    if method != Method::GET {
        return refuse(StatusCode::METHOD_NOT_ALLOWED, "Live data is fetched with GET.");
    }
    {
        let mut recent = ctx.recent.lock().unwrap();
        let now = Instant::now();
        while recent.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            recent.pop_front();
        }
        if recent.len() >= FETCH_PER_MINUTE {
            drop(recent);
            note(&ctx, "live_failed", format!("more than {FETCH_PER_MINUTE} live-data requests a minute: fetch less often (every 15 to 60 seconds), and several things in one request where the source allows it"));
            return refuse(StatusCode::TOO_MANY_REQUESTS, "Too many requests: fetch less often.");
        }
        recent.push_back(now);
    }
    let Some(asked) = asked_url(&uri) else {
        note(&ctx, "live_failed", "a fetch without an address: call /__sulcus/fetch?url= + encodeURIComponent(the full https address)".into());
        return refuse(StatusCode::BAD_REQUEST, "Give the address to fetch as ?url=");
    };
    let shown = |u: &reqwest::Url| format!("{}{}", u.host_str().unwrap_or_default(), u.path());
    if !crate::web::allowed_for_chat(&ctx.state.db.lock().unwrap(), &ctx.chat_id) {
        let what = reqwest::Url::parse(&asked).map(|u| shown(&u)).unwrap_or_else(|_| asked.chars().take(80).collect());
        note(&ctx, "live_blocked", what);
        return refuse(StatusCode::FORBIDDEN, "This chat is offline, so the site can't get live data.");
    }
    let mut url = match crate::web::check_url(&asked).await {
        Ok(u) => u,
        Err(e) => {
            note(&ctx, "live_failed", format!("{}: {e}", asked.chars().take(80).collect::<String>()));
            return refuse(StatusCode::BAD_REQUEST, &e);
        }
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let what = shown(&url);
    let fake = url.query_pairs().find(|(_, v)| placeholder_key(v)).map(|(k, _)| k.into_owned());
    let mut key = ctx.state.cipher().ok().and_then(|c| crate::sitekeys::for_request(&ctx.state.db.lock().unwrap(), &c, &ctx.chat_id, &host));
    if key.is_some() {
        match keyed_url(url.clone()) {
            Some(u) => url = u,
            None => {
                key = None;
                note(&ctx, "live_failed", format!("{what}: the API key is only sent over https, so it was left out; use the https:// address"));
            }
        }
    }
    if let Some(k) = &key {
        add_key(&mut url, k);
    }
    let clients = {
        let mut c = ctx.clients.lock().unwrap();
        if c.is_none() {
            match crate::web::site_clients(&ctx.state.db.lock().unwrap(), &ctx.chat_id) {
                Ok(made) => *c = Some(made),
                Err(e) => {
                    drop(c);
                    note(&ctx, "live_blocked", what.clone());
                    return refuse(StatusCode::FORBIDDEN, &e);
                }
            }
        }
        c.clone().unwrap()
    };
    let client = if key.is_some() { &clients.1 } else { &clients.0 };
    let mut req = client.get(url).header(header::ACCEPT, headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or("application/json, */*"));
    if let Some(k) = key.as_ref().filter(|k| !k.header.is_empty()) {
        req = req.header(k.header.as_str(), k.header_value());
    }
    let mut res = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // Without the address: it may carry the key.
            let e = e.without_url();
            note(&ctx, "live_failed", format!("{what}: couldn't get it ({e})"));
            return refuse(StatusCode::BAD_GATEWAY, &format!("Couldn't reach {host}."));
        }
    };
    let status = res.status();
    let kind = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream").to_string();
    let mut body = Vec::new();
    loop {
        match res.chunk().await {
            Ok(Some(c)) if body.len() + c.len() <= FETCH_BYTES => body.extend_from_slice(&c),
            Ok(Some(_)) => {
                note(&ctx, "live_failed", format!("{what}: the answer was over 5 MB; ask for less"));
                return refuse(StatusCode::BAD_GATEWAY, "That answer is too big.");
            }
            Ok(None) => break,
            Err(e) => {
                let e = e.without_url();
                note(&ctx, "live_failed", format!("{what}: the answer broke off ({e})"));
                return refuse(StatusCode::BAD_GATEWAY, &format!("{host} stopped answering."));
            }
        }
    }
    match status.as_u16() {
        200..=299 => {
            note(&ctx, "live_ok", host.clone());
            if ctx.hosts.lock().unwrap().insert(host.clone()) {
                ctx.state.log("privacy", &format!("A site the assistant built got live data from {host}"));
            }
        }
        401 | 403 if key.is_some() => note(&ctx, "live_key_refused", format!("{host} ({status})")),
        401 | 403 => note(&ctx, "live_key_needed", format!("{what} ({status})")),
        429 => note(&ctx, "live_failed", format!("{what} answered 429 (too many requests): fetch less often")),
        _ => note(&ctx, "live_failed", format!("{what} answered {status}")),
    }
    if let (Some(name), None) = (&fake, &key) {
        note(&ctx, "live_placeholder", format!("{host} (the {name} parameter)"));
    }
    let mut r = (status, body).into_response();
    let h = r.headers_mut();
    if let Ok(v) = kind.parse() {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    // Shown as a page, it would run apart from the site (no reaching its pages or the fetch route).
    h.insert(header::CONTENT_SECURITY_POLICY, "sandbox".parse().unwrap());
    r
}

impl Default for Report {
    fn default() -> Self {
        Report {
            kind: String::new(),
            message: String::new(),
            title: String::new(),
            text: 0,
            buttons: 0,
            inputs: 0,
            missing_ids: Vec::new(),
            ids: Vec::new(),
            early_buttons: 0,
            quiet: Vec::new(),
            popups: Vec::new(),
            labels: Vec::new(),
            changes: None,
            dead: Vec::new(),
        }
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
  // Requests still out (a click that loads data is judged once they're back).
  var inflight = 0;
  window.fetch = function (input) {
    var url = String((input && input.url) || input);
    var p = of.apply(this, arguments);
    inflight++;
    var settle = function () { inflight--; };
    p.then(settle, settle);
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
  // after which nothing on the page changed: no new text, no message, no
  // error (and the ones that only showed a pop-up message).
  var tryClicks = function (els, done) {
    var quiet = [], popups = [];
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
      if (i >= els.length) return done(quiet, popups);
      var el = els[i++];
      var before = text(), d = dialogs, e = errors, waited = 0;
      try { el.click(); } catch (x) {}
      var judge = function () {
        // Data it asked for is still coming (up to 3 s).
        if (inflight > 0 && waited < 3000) { waited += 150; return setTimeout(judge, 150); }
        var same = text() === before;
        if (same && dialogs === d && errors === e) quiet.push(label(el));
        else if (same && dialogs !== d) popups.push(label(el));
        next();
      };
      setTimeout(judge, 150);
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
        labels: controls.concat([].slice.call(document.querySelectorAll('input:not([type=hidden]), select, textarea'))).map(function (el) { return el.placeholder || label(el); }).slice(0, 40),
        early_buttons: early, changes: changes };
      // Clicking takes a while (up to 3 s a button): the checker waits for it.
      send({ kind: 'clicking', message: String(Math.min(live.length, 8)) });
      tryClicks(live.slice(0, 8), function (quiet, popups) {
        summary.quiet = quiet.slice(0, 12);
        summary.popups = popups.slice(0, 12);
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
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut clicking = false;
    while tokio::time::Instant::now() < deadline {
        // Loaded, and trying its buttons: time for each click (a page that
        // loads data after a click got a false "endless loop" at 15 s).
        if !clicking {
            if let Some(n) = reports.lock().unwrap().iter().find(|r| r.kind == "clicking").map(|r| r.message.parse::<u64>().unwrap_or(8)) {
                clicking = true;
                deadline = tokio::time::Instant::now() + Duration::from_millis(n.min(8) * 3_300 + 3_000);
            }
        }
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

/// Things the person asked to do on the page, each with words its control would say.
pub fn asked_actions(asked: &str) -> Vec<(&'static str, &'static [&'static str])> {
    const ACTIONS: &[(&str, &[&str])] = &[
        ("buy", &["buy", "purchase", "order"]),
        ("sell", &["sell"]),
        ("add", &["add", "+", "new", "create", "save"]),
        ("delete", &["delete", "remove", "clear", "×", "✕", "🗑"]),
        ("search", &["search", "find", "go", "🔍"]),
        ("send", &["send", "submit", "post"]),
        ("book", &["book", "reserve"]),
        ("reserve", &["book", "reserve"]),
        ("convert", &["convert", "calculate", "="]),
        ("calculate", &["calculate", "compute", "convert", "="]),
        ("vote", &["vote"]),
        ("upload", &["upload", "choose", "browse"]),
        ("deposit", &["deposit", "add"]),
        ("withdraw", &["withdraw"]),
        ("transfer", &["transfer", "send"]),
    ];
    let low = asked.to_lowercase();
    ACTIONS.iter().filter(|(verb, _)| regex::Regex::new(&format!(r"\b{verb}\b")).is_ok_and(|re| re.is_match(&low))).copied().collect()
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
            // Straight from the page to another site: browsers don't let most data sources be read that way.
            "request_failed" if web && outside(&r.message) => once(
                format!(
                    "Request failed: {}. A page can't read most data sources directly: fetch it through SulcusAI instead, \
                     fetch('/__sulcus/fetch?url=' + encodeURIComponent(address)), and leave any key out (request_api_key)",
                    r.message
                ),
                &mut problems,
            ),
            "request_failed" => once(format!("Request failed: {}", r.message), &mut problems),
            "live_blocked" => once(
                format!("The page asked for live data ({}) but this chat is offline: use simulated data labelled as simulated, or ask to turn the web on with request_web", r.message),
                &mut problems,
            ),
            "live_key_needed" => once(
                format!(
                    "{} refused the request because it needs an API key: call request_api_key so the user can connect it (SulcusAI then adds the key; \
                     never put a key in the files), or switch to a source that needs no key",
                    r.message
                ),
                &mut problems,
            ),
            "live_key_refused" => once(
                format!("{} refused the key the user gave: tell the user the key may be wrong or not allowed for this data, or switch to a source that needs no key", r.message),
                &mut problems,
            ),
            "live_placeholder" => once(
                format!("The site sends a stand-in instead of an API key to {}: take it out of the files and call request_api_key (SulcusAI adds the real key)", r.message),
                &mut problems,
            ),
            "live_failed" => once(format!("Live data failed: {}", r.message), &mut problems),
            "blocked" if !web => once(
                format!("Blocked {} because this chat is offline: use the site's own files, or simulated data labelled as simulated (or ask to turn the web on with request_web)", r.message),
                &mut problems,
            ),
            "blocked" => once(format!("Blocked by the page's own policy: {}", r.message), &mut problems),
            _ => {}
        }
    }
    let mut live: Vec<&str> = Vec::new();
    for r in reports.iter().filter(|r| r.kind == "live_ok") {
        if !live.contains(&r.message.as_str()) {
            live.push(&r.message);
        }
    }
    let summary = reports.iter().rev().find(|r| r.kind == "summary");
    let text = match summary {
        None if reports.iter().any(|r| r.kind == "clicking") => {
            once("The page loaded, but clicking its buttons never finished: one of them may start an endless loop or wait forever".into(), &mut problems);
            "It loaded, but trying its buttons never finished.".to_string()
        }
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
            if !s.popups.is_empty() && wants_actions(asked) {
                once(
                    format!(
                        "Clicking these only shows a pop-up message and changes nothing on the page: {}. Show the result on the page itself \
                         (for example update the cash, the holdings, the totals or a list), and keep pop-ups for errors",
                        s.popups.iter().map(|q| format!("“{q}”")).collect::<Vec<_>>().join(", ")
                    ),
                    &mut problems,
                );
            }
            // Real data that refreshes every 15 s or more won't change in the seconds the check watches.
            if s.changes == Some(false) && live.is_empty() && wants_live(asked) {
                once(
                    if web {
                        "Nothing on the page changes by itself and it got no live data, though the user asked for live updates: fetch the real data \
                         through fetch('/__sulcus/fetch?url=' + encodeURIComponent(address)) and fetch it again every 15 to 30 seconds with setInterval"
                    } else {
                        "Nothing on the page changes by itself, though the user asked for it to update live: change the numbers on a timer (setInterval), \
                         show them, and label them as simulated (this chat is offline)"
                    }
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
            // Things they asked to do that no control on the page offers (the
            // live test's 14B removed Buy and Sell to get rid of an error).
            if s.buttons + s.inputs > 0 && !s.labels.is_empty() {
                let have: Vec<String> = s.labels.iter().map(|l| l.to_lowercase()).collect();
                let missing: Vec<&str> = asked_actions(asked).into_iter().filter(|(_, words)| !have.iter().any(|l| words.iter().any(|w| l.contains(w)))).map(|(v, _)| v).collect();
                if !missing.is_empty() {
                    once(
                        format!(
                            "The user asked to {} on the page, but no button or field does that (it has: {}). Add a working control for each, and keep the features that already work",
                            missing.join(" and "),
                            s.labels.iter().filter(|l| !l.trim().is_empty()).take(12).map(|l| format!("“{l}”")).collect::<Vec<_>>().join(", ")
                        ),
                        &mut problems,
                    );
                }
            }
            if !s.dead.is_empty() {
                once(
                    format!("These buttons do nothing when clicked (no click handler): {}. Make every one of them work", s.dead.iter().map(|d| format!("“{d}”")).collect::<Vec<_>>().join(", ")),
                    &mut problems,
                );
            }
            format!(
                "It loaded: “{}”, {} characters of text, {} buttons ({} with nothing behind them), {} fields.{}",
                if s.title.is_empty() { "(no title)" } else { &s.title },
                s.text,
                s.buttons,
                s.dead.len(),
                s.inputs,
                if live.is_empty() { String::new() } else { format!(" It got live data from {}.", live.join(", ")) }
            )
        }
    };
    (problems, text)
}

/// Puts the line a script error points to beside it ("(/app.js:46)" gets
/// line 46's text), so a small model can fix it with one exact edit.
pub fn quote_lines(root: &Path, problems: &mut [String]) {
    static AT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\((/[^():]+\.(?:js|mjs|html?)):(\d+)\)").unwrap());
    // 'You don't have…': the apostrophe ends the string (the live test's 14B wrote this).
    static APOSTROPHE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"'[^'\n]*[A-Za-z]'[A-Za-z][^'\n]*'").unwrap());
    // "buyStock is not defined" from an onclick: the fix is in the script, not
    // the page (the live test's 14B kept editing the button instead).
    static UNDEFINED: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"ReferenceError: ([A-Za-z_$][\w$]*) is not defined").unwrap());
    let scripts: Vec<String> = std::fs::read_dir(root)
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.to_ascii_lowercase().ends_with(".js")).collect())
        .unwrap_or_default();
    for p in problems.iter_mut() {
        if let Some(name) = UNDEFINED.captures(p).map(|c| c[1].to_string()) {
            let script = scripts.first().map_or("the page's script".to_string(), |s| s.clone());
            p.push_str(&format!(
                ". Nothing defines {name}: write function {name}(…) {{ … }} in {script}, at the top level (not inside another function or a DOMContentLoaded handler), so the page can call it"
            ));
            continue;
        }
        let Some(c) = AT.captures(p) else { continue };
        let (path, n) = (c[1].to_string(), c[2].parse::<usize>().unwrap_or(0));
        let Some(file) = file_for(root, &path) else { continue };
        let Some(line) = std::fs::read_to_string(&file).ok().and_then(|t| t.lines().nth(n.saturating_sub(1)).map(|l| l.trim().to_string())) else { continue };
        if line.is_empty() {
            continue;
        }
        let line: String = line.chars().take(240).collect();
        let hint = if p.contains("SyntaxError") && APOSTROPHE.is_match(&line) {
            " (the apostrophe inside '…' ends the string early: put that text in double quotes or backticks instead)"
        } else {
            ""
        };
        p.push_str(&format!(". Line {n} of {} is: {line}{hint}", path.trim_start_matches('/')));
    }
}

/// A failed request's address is another site's (not this PC's).
fn outside(message: &str) -> bool {
    let address = message.split_whitespace().next().unwrap_or_default();
    (address.starts_with("http://") || address.starts_with("https://")) && crate::ports::local_port(address).is_none() && !address.contains("://127.0.0.1") && !address.contains("://localhost")
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
        // The same rule as the chat's web tools (the Web search feature counts too).
        (crate::projects::chat_folders(&conn, &chat), crate::web::allowed_for_chat(&conn, &chat_id))
    };
    let sandbox = crate::sandbox::Sandbox::new(&folders);
    let path = PathBuf::from(&root);
    if !sandbox.roots().iter().any(|r| dunce::canonicalize(&path).is_ok_and(|p| p.starts_with(r))) {
        return Err("That folder isn't shared with this chat anymore.".into());
    }
    serve(state.inner(), &chat_id, &path, start.as_deref().unwrap_or("index.html"), web).await
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
        assert!(describe(&[bare.clone()], true, "make me a page about my dog Rex").0.is_empty());
        assert!(wants_actions("track my portfolio") && !wants_actions("a page about the history of Rome"));
        // Buy and Sell that only pop up a message (the live stock test's first site).
        let mut popping = bare.clone();
        popping.buttons = 6;
        popping.popups = vec!["Buy".into(), "Sell".into()];
        let (problems, _) = describe(&[popping.clone()], true, "a stock exchange where I can buy and sell");
        assert!(problems.len() == 1 && problems[0].contains("only shows a pop-up"), "{problems:?}");
        assert!(describe(&[popping], true, "a page about my dog Rex").0.is_empty());
        // Buy and Sell taken out, only Refresh left (the live test's 4th site).
        let mut shrunk = bare.clone();
        shrunk.buttons = 1;
        shrunk.labels = vec!["Refresh Prices".into()];
        let (problems, _) = describe(&[shrunk.clone()], true, "a live stock exchange where I can buy and sell shares");
        assert!(problems.len() == 1 && problems[0].contains("asked to buy and sell") && problems[0].contains("“Refresh Prices”"), "{problems:?}");
        shrunk.labels = vec!["Buy shares".into(), "Sell".into(), "Refresh Prices".into()];
        assert!(describe(&[shrunk], true, "a live stock exchange where I can buy and sell shares").0.is_empty());
        // Real prices that refresh every 15 s don't change while the check watches: not a problem.
        let mut real = bare;
        real.buttons = 8;
        real.changes = Some(false);
        let (problems, text) = describe(&[r("live_ok", "query1.finance.yahoo.com"), r("live_ok", "query1.finance.yahoo.com"), real], true, "a live stock exchange where I can buy");
        assert!(problems.is_empty(), "{problems:?}");
        assert!(text.contains("It got live data from query1.finance.yahoo.com."), "{text}");
    }

    #[test]
    fn live_data_problems_point_to_the_fetch_route_and_keys() {
        let r = |kind: &str, message: &str| Report { kind: kind.into(), message: message.into(), ..Report::default() };
        let mut s = r("summary", "");
        s.title = "Stocks".into();
        s.text = 500;
        s.buttons = 2;
        let (problems, _) = describe(
            &[
                r("request_failed", "https://finnhub.io/api/v1/quote?symbol=AAPL (TypeError: Failed to fetch)"),
                r("request_failed", "http://127.0.0.1:5000/data.json (404)"),
                r("live_key_needed", "api.polygon.io/v2/last (401 Unauthorized)"),
                r("live_placeholder", "finnhub.io (the token parameter)"),
                r("live_failed", "example.com/x answered 500 Internal Server Error"),
                s,
            ],
            true,
            "live stock prices",
        );
        assert_eq!(problems.len(), 5, "{problems:?}");
        assert!(problems[0].contains("/__sulcus/fetch") && problems[0].contains("request_api_key"));
        assert!(!problems[1].contains("/__sulcus/fetch"), "a file of the site's own isn't another site");
        assert!(problems[2].contains("request_api_key") && problems[2].contains("api.polygon.io"));
        assert!(problems[3].contains("stand-in"));
        assert!(problems[4].starts_with("Live data failed"));
        // Offline, a direct request is only a failed request (the policy says why).
        let (problems, _) = describe(&[r("request_failed", "https://finnhub.io/x (Failed to fetch)")], false, "");
        assert!(!problems[0].contains("/__sulcus/fetch"));
    }

    #[test]
    fn a_script_error_comes_with_its_line() {
        let dir = std::env::temp_dir().join(format!("sulcus-preview-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("app.js"), "let cash = 1;\n  alert('You don't have any shares to sell.');\n").unwrap();
        let root = dunce::canonicalize(&dir).unwrap();
        let mut problems = vec!["Script error: Uncaught SyntaxError: missing ) after argument list (/app.js:2)".to_string(), "Missing file (404): /x.css".to_string()];
        quote_lines(&root, &mut problems);
        assert!(problems[0].ends_with("Line 2 of app.js is: alert('You don't have any shares to sell.'); (the apostrophe inside '…' ends the string early: put that text in double quotes or backticks instead)"), "{}", problems[0]);
        assert_eq!(problems[1], "Missing file (404): /x.css");
        // A button calling a function nothing defines: the fix goes in the script.
        let mut problems = vec!["Script error: Uncaught ReferenceError: buyStock is not defined (/index.html:17)".to_string()];
        quote_lines(&root, &mut problems);
        assert!(problems[0].contains("write function buyStock(…) { … } in app.js, at the top level"), "{}", problems[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stand_in_keys_are_noticed_and_real_keys_replace_them() {
        for fake in ["YOUR_API_KEY", "your-key-here", "API_KEY", "apikey", "<key>", "{token}", "xxxxxxxx", "demo"] {
            assert!(placeholder_key(fake), "{fake}");
        }
        for real in ["AAPL", "c1a2b3c4d5e6f7", "1m", "usd", "bitcoin,ethereum"] {
            assert!(!placeholder_key(real), "{real}");
        }
        let k = crate::sitekeys::SiteKey { host: "finnhub.io".into(), platform: "Finnhub".into(), param: "token".into(), header: String::new(), key: "s3cr3t".into(), chats: vec![] };
        let mut u = reqwest::Url::parse("https://finnhub.io/api/v1/quote?symbol=AAPL&token=YOUR_API_KEY").unwrap();
        add_key(&mut u, &k);
        assert_eq!(u.as_str(), "https://finnhub.io/api/v1/quote?symbol=AAPL&token=s3cr3t");
        // A key never goes over plain http: the address goes over https instead, or the key stays here.
        let keyed = |s: &str| keyed_url(reqwest::Url::parse(s).unwrap()).map(|u| u.to_string());
        assert_eq!(keyed("http://finnhub.io/api/v1/quote?symbol=AAPL").as_deref(), Some("https://finnhub.io/api/v1/quote?symbol=AAPL"));
        assert_eq!(keyed("https://finnhub.io/x").as_deref(), Some("https://finnhub.io/x"));
        assert_eq!(keyed("http://api.example.com:8080/x"), None);
        assert_eq!(asked_url(&"/__sulcus/fetch?url=https%3A%2F%2Fa.io%2Fq%3Fs%3D1%26t%3D2".parse().unwrap()).as_deref(), Some("https://a.io/q?s=1&t=2"));
        assert_eq!(asked_url(&"/__sulcus/fetch".parse().unwrap()), None);
        // Not encoded (what Qwen2.5-Coder 14B wrote in the live test): the whole address still arrives.
        assert_eq!(
            asked_url(&"/__sulcus/fetch?url=https://query1.finance.yahoo.com/v8/finance/chart/AAPL?interval=1m&range=1d".parse().unwrap()).as_deref(),
            Some("https://query1.finance.yahoo.com/v8/finance/chart/AAPL?interval=1m&range=1d")
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_site_is_served_on_this_pc_only_and_follows_the_chats_web() {
        let state = crate::e2e::temp_state(None);
        let serve = |chat: &'static str, dir: PathBuf, web: bool| {
            let state = state.clone();
            async move { super::serve(&state, chat, &dir, "index.html", web).await }
        };
        let dir = std::env::temp_dir().join(format!("sulcus-preview-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><title>T</title><p>Hello</p>").unwrap();
        let s = serve("chat-a", dir.clone(), false).await.unwrap();
        assert!(s.url.starts_with("http://127.0.0.1:") && s.url.ends_with("/index.html"));
        // The same folder in the same chat is the same preview.
        assert_eq!(serve("chat-a", dir.clone(), false).await.unwrap().id, s.id);
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
        let s2 = serve("chat-a", dir.clone(), true).await.unwrap();
        assert_ne!(s2.id, s.id);
        let r = client.get(&s2.url).send().await.unwrap();
        assert!(r.headers().get("content-security-policy").is_none());
        // The chat goes offline while it's served: its next page gets the offline policy.
        crate::features::set(&state.db.lock().unwrap(), crate::features::Feature::WebSearch, false).unwrap();
        let r = client.get(&s2.url).send().await.unwrap();
        assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("connect-src 'self'"));
        crate::features::set(&state.db.lock().unwrap(), crate::features::Feature::WebSearch, true).unwrap();
        // A big file comes whole, sent in pieces.
        let big: Vec<u8> = (0..3 * 1024 * 1024 + 5).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("big.bin"), &big).unwrap();
        let r = client.get(s2.url.replace("index.html", "big.bin")).send().await.unwrap();
        assert_eq!(r.headers()["content-length"].to_str().unwrap(), big.len().to_string());
        assert!(r.bytes().await.unwrap().as_ref() == big.as_slice(), "the same bytes");
        // Live data: only for the site's own pages, only public addresses, and an address is needed.
        let base = s2.url.trim_end_matches("index.html");
        let r = client.get(format!("{base}__sulcus/fetch?url=https%3A%2F%2Fexample.com%2F")).header("sec-fetch-site", "cross-site").send().await.unwrap();
        assert_eq!(r.status(), 403);
        let r = client.get(format!("{base}__sulcus/fetch")).send().await.unwrap();
        assert_eq!(r.status(), 400);
        assert!(r.text().await.unwrap().contains("\"error\""));
        let r = client.get(format!("{base}__sulcus/fetch?url=http%3A%2F%2F127.0.0.1%3A9%2Fsecret")).send().await.unwrap();
        assert_eq!(r.status(), 400, "nothing on this PC or the home network");
        {
            let sites = SITES.lock().unwrap();
            let kinds: Vec<String> = sites[&s2.id].reports.lock().unwrap().iter().map(|r| r.kind.clone()).collect();
            assert_eq!(kinds, vec!["live_failed".to_string(), "live_failed".to_string()]);
        }
        stop_chat("chat-a");
        assert!(!stop(&s2.id));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

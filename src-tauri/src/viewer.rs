// SPDX-License-Identifier: AGPL-3.0-only
//! The web viewer: web pages inside SulcusAI's own window, under an address
//! bar, to read something without leaving the app. Each chat has its own
//! (docked beside it: its page, history and place stay its own), and the Web
//! page has one too ("main"). Each is a webview laid over the page's empty
//! area: the page says where that area is (`viewer_show`, `viewer_area`) and
//! hides it when it leaves or a dialog opens, since the viewer would cover
//! anything drawn there. One shows at a time; the four used last stay
//! loaded, and an older one closes and opens again on the page it was on.
//! They share a profile (`viewer`), apart from the app's own data.
//!
//! A chat's assistant browses in its chat's viewer (tools::browser), so the
//! person watches it work there and can take over.
//!
//! A chat can be open in a window of its own (side_windows): its viewer
//! lives in the window that last showed it, and moves (reparents) to
//! whichever window shows it next.
//!
//! Safety, as for the assistant's browser: only http and https; never this
//! PC or the home network, unless the person typed that address themselves;
//! downloads go through the main browser; new windows open in place.
//!
//! Every command runs off the main thread: adding a webview waits for the
//! main thread, so from it (a plain command) it would wait forever.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use serde::Deserialize;
use serde_json::json;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl, Window};

use crate::{AppState, AppStateRef};

const PREFIX: &str = "viewer-";
/// Viewers kept loaded.
const MOST: usize = 4;

/// Where the viewer goes, in the window's own (CSS) pixels.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Area {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Keys of the viewers loaded, the one used last at the end.
static RECENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The page a closed viewer was on, to open it there again.
static LAST: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);
/// Hosts on this PC or the home network the person typed in a viewer (by key).
static TYPED: LazyLock<Mutex<HashMap<String, HashSet<String>>>> = LazyLock::new(Default::default);
/// The window each viewer was last shown in (by key), where its assistant's opens too.
static HOSTS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// The window a viewer belongs in: the one given, else where it was last
/// shown (while that's open), else the main window.
fn host(app: &AppHandle, key: &str, given: Option<&Window>) -> Result<Window, String> {
    if let Some(w) = given {
        return Ok(w.clone());
    }
    let last = HOSTS.lock().unwrap().get(key).cloned();
    last.and_then(|l| app.get_window(&l)).or_else(|| app.get_window("main")).ok_or_else(|| "SulcusAI's window isn't open.".to_string())
}

/// A viewer's key: its chat's id, or "main" for the Web page.
pub fn key_of(chat: Option<&str>) -> String {
    let k: String = chat.unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect();
    if k.is_empty() { "main".into() } else { k }
}

fn label(key: &str) -> String {
    format!("{PREFIX}{key}")
}

pub fn webview(app: &AppHandle, key: &str) -> Option<Webview> {
    app.get_webview(&label(key))
}

/// Every viewer open now (they share one profile, so one sign-in).
pub fn all(app: &AppHandle) -> Vec<Webview> {
    app.webviews().into_iter().filter(|(l, _)| l.starts_with(PREFIX)).map(|(_, w)| w).collect()
}

fn place(w: &Webview, a: Area) -> Result<(), String> {
    w.set_position(LogicalPosition::new(a.x, a.y)).map_err(|e| e.to_string())?;
    w.set_size(LogicalSize::new(a.width.max(1.0), a.height.max(1.0))).map_err(|e| e.to_string())
}

/// Whether a viewer may go to this address (checked on every navigation).
fn allowed(key: &str, url: &tauri::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return url.as_str() == "about:blank";
    }
    if crate::browser::navigable(url) {
        return true;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    TYPED.lock().unwrap().get(key).is_some_and(|hosts| hosts.contains(&host))
}

/// The person typed (or opened) this address: its host is allowed in this
/// viewer even when it's on this PC or the home network.
fn typed(key: &str, url: &str) {
    if let Some(host) = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)) {
        TYPED.lock().unwrap().entry(key.to_string()).or_default().insert(host);
    }
}

/// Used now: the oldest beyond the four kept close (and open again later
/// where they were).
fn touch(app: &AppHandle, key: &str) {
    let old: Vec<String> = {
        let mut recent = RECENT.lock().unwrap();
        recent.retain(|k| k != key);
        recent.push(key.to_string());
        let over = recent.len().saturating_sub(MOST);
        recent.drain(..over).collect()
    };
    for k in old {
        if let Some(w) = webview(app, &k) {
            if let Ok(u) = w.url() {
                LAST.lock().unwrap().insert(k.clone(), u.to_string());
            }
            let _ = w.close();
        }
    }
}

/// The viewer for `key`, opened (hidden, out of sight, when there's no
/// `area` yet) if it isn't already, in `window` (moved there when it's in
/// another). Returns it, and whether it's new.
pub fn ensure(app: &AppHandle, state: &AppState, key: &str, url: Option<&str>, area: Option<Area>, window: Option<&Window>) -> Result<(Webview, bool), String> {
    crate::offline::check(crate::offline::Keep::Web)?;
    let window = host(app, key, window)?;
    if let Some(w) = webview(app, key) {
        if w.window().label() != window.label() {
            w.reparent(&window).map_err(|e| format!("Couldn't move the web viewer: {e}"))?;
        }
        touch(app, key);
        return Ok((w, false));
    }
    let profile = state.paths.data.join("viewer");
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let home = crate::browsers::search(&state.db.lock().unwrap()).home();
    let start = url.map(str::to_string).or_else(|| LAST.lock().unwrap().remove(key)).unwrap_or_else(|| home.into());
    let start: tauri::Url = start.parse().map_err(|_| "That isn't a web address.".to_string())?;
    let (k1, k2, k3, k4, k5) = (key.to_string(), key.to_string(), key.to_string(), key.to_string(), key.to_string());
    let (opener, loads, titles) = (app.clone(), app.clone(), app.clone());
    let builder = WebviewBuilder::new(label(key), WebviewUrl::External(start))
        .data_directory(profile)
        .on_navigation(move |u| allowed(&k1, u))
        // Pop-ups and links that open a new window open here instead.
        .on_new_window(move |u, _| {
            if allowed(&k2, &u) {
                if let Some(w) = webview(&opener, &k3) {
                    let _ = w.navigate(u);
                }
            }
            NewWindowResponse::Deny
        })
        .on_page_load(move |_, p| {
            let _ = loads.emit("viewer:page", json!({ "key": k4, "url": p.url().as_str(), "loading": p.event() == PageLoadEvent::Started }));
        })
        .on_document_title_changed(move |_, title| {
            let _ = titles.emit("viewer:title", json!({ "key": k5, "title": title }));
        })
        // Downloads go through the main browser ("Open in Chrome").
        .on_download(|_, _| false);
    // Opened for the assistant before the person looks: far out of sight,
    // at a usual size, so pages lay out as they would on screen.
    let (pos, size) = match area {
        Some(a) => (LogicalPosition::new(a.x, a.y), LogicalSize::new(a.width.max(1.0), a.height.max(1.0))),
        None => (LogicalPosition::new(-20000.0, 0.0), LogicalSize::new(1100.0, 800.0)),
    };
    let w = window.add_child(builder, pos, size).map_err(|e| format!("Couldn't open the web viewer: {e}"))?;
    if area.is_none() {
        let _ = w.hide();
    }
    touch(app, key);
    state.log("network", if key == "main" { "Opened the web viewer" } else { "Opened a chat's web viewer" });
    Ok((w, true))
}

/// A chat's viewer for its assistant: opened out of sight if it isn't, and
/// the chat is told so it can show it beside the conversation.
pub fn for_assistant(app: &AppHandle, state: &AppState, chat_id: &str) -> Result<Webview, String> {
    let key = key_of(Some(chat_id));
    let (w, _) = ensure(app, state, &key, None, None, None)?;
    let _ = app.emit("viewer:dock", json!({ "key": key }));
    Ok(w)
}

/// Closes every viewer (going offline).
pub fn close_all(app: &AppHandle) {
    for w in all(app) {
        let _ = w.close();
    }
    RECENT.lock().unwrap().clear();
}

/// What was typed in the address bar: a web address as it is, a name like
/// "example.com" with https in front, anything else a search (with the
/// search picked in Settings).
pub fn address(typed: &str, search: crate::browsers::Search) -> String {
    let t = typed.trim();
    if t.is_empty() {
        return search.home().into();
    }
    if let Ok(u) = reqwest::Url::parse(t) {
        if matches!(u.scheme(), "http" | "https") {
            return u.to_string();
        }
    }
    let host = t.split(['/', '?', '#']).next().unwrap_or("");
    // Servers on this PC (a site the assistant built) speak plain http.
    let local = host.starts_with("localhost") || host.starts_with("127.0.0.1");
    let named = !t.contains(char::is_whitespace) && (local || host.contains('.') && !host.starts_with('.') && !host.ends_with('.'));
    if named {
        if let Ok(u) = reqwest::Url::parse(&format!("{}://{t}", if local { "http" } else { "https" })) {
            return u.to_string();
        }
    }
    search.of(t)
}

// ---------- commands (`key`: a chat's id, or none for the Web page) ----------

/// Shows a viewer over `area` of the window asking (at `url` when given),
/// opening it there or moving it there if needed; any other viewer in that
/// window goes out of sight.
#[tauri::command(async)]
pub fn viewer_show(app: AppHandle, window: Window, state: AppStateRef<'_>, key: Option<String>, url: Option<String>, area: Area) -> Result<(), String> {
    if !crate::net::allowed(state.settings().connectivity, crate::net::Purpose::Web, false) {
        return Err("The web viewer needs Local AI + Web. Switch to it from the status bar.".into());
    }
    let key = key_of(key.as_deref());
    let search = crate::browsers::search(&state.db.lock().unwrap());
    let target = url.map(|u| address(&u, search));
    if let Some(u) = &target {
        typed(&key, u);
    }
    HOSTS.lock().unwrap().insert(key.clone(), window.label().to_string());
    for w in all(&app) {
        if w.label() != label(&key) && w.window().label() == window.label() {
            let _ = w.hide();
        }
    }
    let (w, fresh) = ensure(&app, &state, &key, target.as_deref(), Some(area), Some(&window))?;
    place(&w, area)?;
    w.show().map_err(|e| e.to_string())?;
    if let (Some(u), false) = (target, fresh) {
        w.navigate(u.parse().map_err(|_| "That isn't a web address.".to_string())?).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The page's empty area moved or changed size.
#[tauri::command(async)]
pub fn viewer_area(app: AppHandle, key: Option<String>, area: Area) -> Result<(), String> {
    match webview(&app, &key_of(key.as_deref())) {
        Some(w) => place(&w, area),
        None => Ok(()),
    }
}

/// Out of sight (another page, a dialog over it, or minimized), kept as it
/// was. No key: every viewer in the window asking.
#[tauri::command(async)]
pub fn viewer_hide(app: AppHandle, window: Window, key: Option<String>) -> Result<(), String> {
    for w in all(&app) {
        let mine = match &key {
            None => w.window().label() == window.label(),
            Some(_) => w.label() == label(&key_of(key.as_deref())),
        };
        if mine {
            w.hide().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Back, forward, reload, or an address typed in the bar.
#[tauri::command(async)]
pub fn viewer_go(app: AppHandle, state: AppStateRef<'_>, key: Option<String>, to: String) -> Result<(), String> {
    let key = key_of(key.as_deref());
    let w = webview(&app, &key).ok_or("The web viewer isn't open.")?;
    match to.as_str() {
        "back" => w.eval("history.back()"),
        "forward" => w.eval("history.forward()"),
        "reload" => w.reload(),
        typed_in => {
            let u = address(typed_in, crate::browsers::search(&state.db.lock().unwrap()));
            typed(&key, &u);
            w.navigate(u.parse().map_err(|_| "That isn't a web address.".to_string())?)
        }
    }
    .map_err(|e| e.to_string())
}

/// The address it's on: open, or closed and kept to open again.
#[tauri::command(async)]
pub fn viewer_url(app: AppHandle, key: Option<String>) -> Option<String> {
    let key = key_of(key.as_deref());
    webview(&app, &key).and_then(|w| w.url().ok()).map(|u| u.to_string()).or_else(|| LAST.lock().unwrap().get(&key).cloned())
}

/// Whether a chat has a viewer (open, or closed and kept to open again).
#[tauri::command(async)]
pub fn viewer_exists(app: AppHandle, key: Option<String>) -> bool {
    let key = key_of(key.as_deref());
    webview(&app, &key).is_some() || LAST.lock().unwrap().contains_key(&key)
}

/// Closes it (its Close): the next one starts fresh.
#[tauri::command(async)]
pub fn viewer_close(app: AppHandle, key: Option<String>) -> Result<(), String> {
    let key = key_of(key.as_deref());
    LAST.lock().unwrap().remove(&key);
    TYPED.lock().unwrap().remove(&key);
    HOSTS.lock().unwrap().remove(&key);
    RECENT.lock().unwrap().retain(|k| k != &key);
    match webview(&app, &key) {
        Some(w) => w.close().map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_becomes_an_address_or_a_search() {
        use crate::browsers::Search;
        let ddg = Search::Duckduckgo;
        assert_eq!(address("https://example.com/a?b=1", ddg), "https://example.com/a?b=1");
        assert_eq!(address("example.com", ddg), "https://example.com/");
        assert_eq!(address("news.ycombinator.com/item?id=1", ddg), "https://news.ycombinator.com/item?id=1");
        assert_eq!(address("localhost:5173", ddg), "http://localhost:5173/");
        assert_eq!(address("nvda earnings date", ddg), "https://duckduckgo.com/?q=nvda+earnings+date");
        assert_eq!(address("file:///C:/secret.txt", ddg), "https://duckduckgo.com/?q=file%3A%2F%2F%2FC%3A%2Fsecret.txt");
        assert_eq!(address("  ", ddg), "https://duckduckgo.com/");
        // Searched with the one picked in Settings.
        assert_eq!(address("nvda earnings date", Search::Google), "https://www.google.com/search?q=nvda+earnings+date");
        assert_eq!(address("weather", Search::Bing), "https://www.bing.com/search?q=weather");
        assert_eq!(address("", Search::Brave), "https://search.brave.com/");
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! The web viewer: web pages inside SulcusAI's own window, under an address
//! bar (the Web page), to read something without leaving the app. It's a
//! second webview laid over the page's empty area: the page says where that
//! area is (`viewer_show`, `viewer_area`) and hides it when it leaves or a
//! dialog opens, since the viewer would cover anything drawn there. Its own
//! profile (`viewer`), apart from the assistant's browser. The globe opens
//! the main browser (Chrome when nothing else was picked) instead.
//!
//! Every command runs off the main thread: adding a webview waits for the
//! main thread, so from it (a plain command) it would wait forever.

use serde::Deserialize;
use serde_json::json;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl};

use crate::AppStateRef;

const LABEL: &str = "viewer";
const HOME: &str = "https://duckduckgo.com/";

/// Where the viewer goes, in the window's own (CSS) pixels.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Area {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn webview(app: &AppHandle) -> Option<Webview> {
    app.get_webview(LABEL)
}

fn place(w: &Webview, a: Area) -> Result<(), String> {
    w.set_position(LogicalPosition::new(a.x, a.y)).map_err(|e| e.to_string())?;
    w.set_size(LogicalSize::new(a.width.max(1.0), a.height.max(1.0))).map_err(|e| e.to_string())
}

/// What was typed in the address bar: a web address as it is, a name like
/// "example.com" with https in front, anything else a search.
pub fn address(typed: &str) -> String {
    let t = typed.trim();
    if t.is_empty() {
        return HOME.into();
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
    reqwest::Url::parse_with_params(HOME, &[("q", t)]).map(|u| u.to_string()).unwrap_or_else(|_| HOME.into())
}

/// Opens the viewer over `area` (at `url` when given), or shows it again.
#[tauri::command(async)]
pub fn viewer_show(app: AppHandle, state: AppStateRef<'_>, url: Option<String>, area: Area) -> Result<(), String> {
    crate::offline::check(crate::offline::Keep::Web)?;
    if !crate::net::allowed(state.settings().connectivity, crate::net::Purpose::Web, false) {
        return Err("The web viewer needs Local AI + Web. Switch to it from the status bar.".into());
    }
    let target = url.map(|u| address(&u));
    if let Some(w) = webview(&app) {
        place(&w, area)?;
        w.show().map_err(|e| e.to_string())?;
        if let Some(u) = target {
            w.navigate(u.parse().map_err(|_| "That isn't a web address.".to_string())?).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let window = app.get_window("main").ok_or("SulcusAI's window isn't open.")?;
    let profile = state.paths.data.join("viewer");
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let start = target.unwrap_or_else(|| HOME.into());
    let (opener, loads, titles) = (app.clone(), app.clone(), app.clone());
    let builder = WebviewBuilder::new(LABEL, WebviewUrl::External(start.parse().map_err(|_| "That isn't a web address.".to_string())?))
        .data_directory(profile)
        .on_navigation(|u| matches!(u.scheme(), "http" | "https"))
        // Pop-ups and links that open a new window open here instead.
        .on_new_window(move |u, _| {
            if matches!(u.scheme(), "http" | "https") {
                if let Some(w) = webview(&opener) {
                    let _ = w.navigate(u);
                }
            }
            NewWindowResponse::Deny
        })
        .on_page_load(move |_, p| {
            let _ = loads.emit("viewer:page", json!({ "url": p.url().as_str(), "loading": p.event() == PageLoadEvent::Started }));
        })
        .on_document_title_changed(move |_, title| {
            let _ = titles.emit("viewer:title", json!({ "title": title }));
        })
        // Downloads go through the main browser ("Open in Chrome").
        .on_download(|_, _| false);
    window
        .add_child(builder, LogicalPosition::new(area.x, area.y), LogicalSize::new(area.width.max(1.0), area.height.max(1.0)))
        .map_err(|e| format!("Couldn't open the web viewer: {e}"))?;
    state.log("network", "Opened the web viewer");
    Ok(())
}

/// The page's empty area moved or changed size.
#[tauri::command(async)]
pub fn viewer_area(app: AppHandle, area: Area) -> Result<(), String> {
    match webview(&app) {
        Some(w) => place(&w, area),
        None => Ok(()),
    }
}

/// Out of sight (another page, or a dialog over the page), kept as it was.
#[tauri::command(async)]
pub fn viewer_hide(app: AppHandle) -> Result<(), String> {
    match webview(&app) {
        Some(w) => w.hide().map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

/// Back, forward, reload, or an address typed in the bar.
#[tauri::command(async)]
pub fn viewer_go(app: AppHandle, to: String) -> Result<(), String> {
    let w = webview(&app).ok_or("The web viewer isn't open.")?;
    match to.as_str() {
        "back" => w.eval("history.back()"),
        "forward" => w.eval("history.forward()"),
        "reload" => w.reload(),
        typed => w.navigate(address(typed).parse().map_err(|_| "That isn't a web address.".to_string())?),
    }
    .map_err(|e| e.to_string())
}

/// The address it's on, if it's open.
#[tauri::command(async)]
pub fn viewer_url(app: AppHandle) -> Option<String> {
    webview(&app).and_then(|w| w.url().ok()).map(|u| u.to_string())
}

/// Closes it (the page's Close).
#[tauri::command(async)]
pub fn viewer_close(app: AppHandle) -> Result<(), String> {
    match webview(&app) {
        Some(w) => w.close().map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_becomes_an_address_or_a_search() {
        assert_eq!(address("https://example.com/a?b=1"), "https://example.com/a?b=1");
        assert_eq!(address("example.com"), "https://example.com/");
        assert_eq!(address("news.ycombinator.com/item?id=1"), "https://news.ycombinator.com/item?id=1");
        assert_eq!(address("localhost:5173"), "http://localhost:5173/");
        assert_eq!(address("nvda earnings date"), "https://duckduckgo.com/?q=nvda+earnings+date");
        assert_eq!(address("file:///C:/secret.txt"), "https://duckduckgo.com/?q=file%3A%2F%2F%2FC%3A%2Fsecret.txt");
        assert_eq!(address("  "), HOME);
    }
}

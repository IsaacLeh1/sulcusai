// SPDX-License-Identifier: AGPL-3.0-only
//! Chats, the Studio and song studio projects in windows of their own,
//! beside the main one, as People has: the same app page, which shows only
//! that when its window is one of these (main.tsx reads the label). Each
//! chat window keeps its own model, so several model sessions can be open
//! side by side (local models still take turns, `engine::Ticket`). They're
//! in the app's capability like the main window, and close with it.

use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Labels: "chat-<chat id>", "studio-<n>", "songstudio-<project id>".
pub const CHAT: &str = "chat-";
pub const STUDIO: &str = "studio-";
pub const SONG: &str = "songstudio-";

/// An id as a window label may carry it.
fn safe(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect()
}

/// Brings a window forward; false when it isn't open.
fn focus(app: &AppHandle, label: &str) -> bool {
    match app.get_webview_window(label) {
        Some(w) => {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            true
        }
        None => false,
    }
}

fn build(app: &AppHandle, label: &str, title: &str, size: (f64, f64)) -> Result<(), String> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(size.0, size.1)
        .min_inner_size(520.0, 400.0)
        .build()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open the window: {e}"))
}

/// A chat in a window of its own (brought forward when it's open already).
/// Async: a window built from a plain command would wait on the very
/// thread that builds it.
#[tauri::command]
pub async fn open_chat_window(app: AppHandle, chat_id: String) -> Result<(), String> {
    let id = safe(&chat_id);
    if id.is_empty() {
        return Err("Pick a chat first.".into());
    }
    let label = format!("{CHAT}{id}");
    if focus(&app, &label) {
        return Ok(());
    }
    build(&app, &label, "Chat - SulcusAI", (1100.0, 800.0))
}

/// The Studio (pictures, video, music and the rest) in a new window of its
/// own; each call opens another.
#[tauri::command]
pub async fn open_studio_window(app: AppHandle) -> Result<(), String> {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let label = loop {
        let l = format!("{STUDIO}{}", NEXT.fetch_add(1, Ordering::Relaxed));
        if app.get_webview_window(&l).is_none() {
            break l;
        }
    };
    build(&app, &label, "Studio - SulcusAI", (1280.0, 860.0))
}

/// A song studio project in a window of its own, or the one it's open in
/// brought forward. With `only_if_open`, nothing opens: true says it was
/// open (so the main window doesn't open it a second time).
#[tauri::command]
pub async fn open_song_studio_window(app: AppHandle, project_id: String, only_if_open: Option<bool>) -> Result<bool, String> {
    let id = safe(&project_id);
    if id.is_empty() {
        return Err("Pick a project first.".into());
    }
    let label = format!("{SONG}{id}");
    if focus(&app, &label) {
        return Ok(true);
    }
    if only_if_open.unwrap_or(false) {
        return Ok(false);
    }
    build(&app, &label, "Song studio - SulcusAI", (1400.0, 900.0))?;
    Ok(true)
}

/// The main window brought forward on a page ("models", "features"), for a
/// window of its own that has no such page.
#[tauri::command]
pub async fn show_main_page(app: AppHandle, page: String) -> Result<(), String> {
    if !focus(&app, "main") {
        return Err("SulcusAI's main window isn't open.".into());
    }
    app.emit_to("main", "app:navigate", json!({ "page": page })).map_err(|e| e.to_string())
}

/// Whether a window is one of these.
pub fn is_side(label: &str) -> bool {
    label.starts_with(CHAT) || label.starts_with(STUDIO) || label.starts_with(SONG)
}

/// The main window closing for good takes these with it.
pub fn on_main_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
        for (label, w) in window.app_handle().webview_windows() {
            if is_side(&label) {
                let _ = w.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_carry_only_safe_ids() {
        assert_eq!(safe("c1a2-b3_x"), "c1a2-b3_x");
        assert_eq!(safe("../../evil id?"), "evilid");
        assert!(is_side("chat-abc") && is_side("studio-2") && is_side("songstudio-p1"));
        assert!(!is_side("main") && !is_side("people") && !is_side("viewer-abc"));
    }
}

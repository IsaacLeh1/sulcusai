// SPDX-License-Identifier: AGPL-3.0-only
//! People in a window of its own, beside the main one: the same app page,
//! which shows only People when its window is this one (main.tsx). It's in
//! the app's capability like the main window, and closes with it.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

pub const LABEL: &str = "people";

/// Opens the People window, or brings it forward when it's open.
/// (Async: a window built from a plain command would wait on the very
/// thread that builds it.)
#[tauri::command]
pub async fn open_people_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::App("index.html".into()))
        .title("People - SulcusAI")
        .inner_size(1000.0, 720.0)
        .min_inner_size(520.0, 400.0)
        .build()
        .map_err(|e| format!("Couldn't open the People window: {e}"))?;
    Ok(())
}

/// The main window closing for good takes the People window with it.
pub fn on_main_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
        if let Some(w) = window.app_handle().get_webview_window(LABEL) {
            let _ = w.close();
        }
    }
}

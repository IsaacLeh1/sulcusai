// SPDX-License-Identifier: AGPL-3.0-only
//! Quick ask: Ctrl+Alt+Space opens a small box from anywhere to ask the
//! assistant something, or act on the text you copied. A tray icon keeps
//! the app reachable, and while this is on, closing the window keeps the
//! app running in the tray so the hotkey still works.
//!
//! Off by default (the Quick ask feature). The box is the app's own page in
//! a second window ("quick"); its chats are ordinary chats.

use std::sync::Arc;

use serde_json::json;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::features::Feature;
use crate::{AppState, AppStateRef};

pub const LABEL: &str = "quick";
pub const HOTKEY: &str = "ctrl+alt+Space";
const TRAY: &str = "main";

pub fn enabled(app: &AppHandle) -> bool {
    let state = app.state::<Arc<AppState>>();
    let conn = state.db.lock().unwrap();
    crate::features::is_on(&conn, Feature::QuickAsk)
}

/// The global-shortcut plugin, calling `toggle` on the hotkey.
pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                toggle(app);
            }
        })
        .build()
}

/// Turns the hotkey and tray icon on or off to match the feature.
pub fn apply(app: &AppHandle) {
    let on = enabled(app);
    let gs = app.global_shortcut();
    if on {
        if !gs.is_registered(HOTKEY) {
            if let Err(e) = gs.register(HOTKEY) {
                eprintln!("quick ask hotkey: {e}");
                app.emit("quick:hotkey_taken", json!({ "hotkey": "Ctrl+Alt+Space" })).ok();
            }
        }
        if app.tray_by_id(TRAY).is_none() {
            if let Err(e) = tray(app) {
                eprintln!("tray: {e}");
            }
        }
    } else {
        let _ = gs.unregister(HOTKEY);
        let _ = app.remove_tray_by_id(TRAY);
        if let Some(w) = app.get_webview_window(LABEL) {
            let _ = w.close();
        }
    }
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let ask = MenuItem::with_id(app, "quick", "Quick ask   Ctrl+Alt+Space", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open SulcusAI", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&ask, &open, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut b = TrayIconBuilder::with_id(TRAY).tooltip("SulcusAI").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon().cloned() {
        b = b.icon(icon);
    }
    b.on_menu_event(|app, e| match e.id.as_ref() {
        "quick" => show(app),
        "open" => show_main(app),
        "quit" => app.exit(0),
        _ => {}
    })
    .on_tray_icon_event(|tray, e| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            show_main(tray.app_handle());
        }
    })
    .build(app)?;
    Ok(())
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Keeps the app in the tray when the main window is closed, while Quick
/// ask is on (so the hotkey keeps working).
pub fn on_main_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        if enabled(window.app_handle()) {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}

pub fn toggle(app: &AppHandle) {
    match app.get_webview_window(LABEL) {
        Some(w) if w.is_visible().unwrap_or(false) => {
            let _ = w.hide();
        }
        _ => show(app),
    }
}

pub fn show(app: &AppHandle) {
    let w = match app.get_webview_window(LABEL) {
        Some(w) => w,
        None => {
            let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
                .title("Quick ask")
                .inner_size(680.0, 150.0)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .transparent(true)
                .shadow(true)
                .visible(false)
                .build();
            match built {
                Ok(w) => {
                    let hide = w.clone();
                    w.on_window_event(move |e| {
                        // Click away and it gets out of the way.
                        if let WindowEvent::Focused(false) = e {
                            let _ = hide.hide();
                        }
                    });
                    w
                }
                Err(e) => {
                    eprintln!("quick ask window: {e}");
                    return;
                }
            }
        }
    };
    // Centered, in the upper part of the screen the cursor is on.
    if let Ok(Some(m)) = w.current_monitor() {
        let (size, pos, scale) = (m.size(), m.position(), m.scale_factor());
        let width = (680.0 * scale) as i32;
        let x = pos.x + (size.width as i32 - width) / 2;
        let y = pos.y + (size.height as f64 * 0.18) as i32;
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = w.show();
    let _ = w.set_focus();
    app.emit_to(LABEL, "quick:shown", json!({})).ok();
}

/// Text on the clipboard, if any (for "use what I copied").
#[cfg(windows)]
fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: the clipboard is opened, read and closed here; the locked
    // memory is only read while locked.
    unsafe {
        IsClipboardFormatAvailable(CF_UNICODETEXT).ok()?;
        OpenClipboard(None).ok()?;
        let text = (|| {
            let h: HANDLE = GetClipboardData(CF_UNICODETEXT).ok()?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut len = 0;
            while *p.add(len) != 0 && len < 400_000 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let _ = GlobalUnlock(g);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

#[cfg(not(windows))]
fn clipboard_text() -> Option<String> {
    None
}

// ---------- commands ----------

#[tauri::command]
pub fn quick_clipboard() -> Option<String> {
    clipboard_text().map(|t| t.trim().chars().take(20_000).collect::<String>()).filter(|t| !t.is_empty())
}

/// The box starts small and grows to fit an answer.
#[tauri::command]
pub fn quick_resize(app: AppHandle, height: f64) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.set_size(tauri::LogicalSize::new(680.0, height.clamp(60.0, 640.0)));
    }
}

#[tauri::command]
pub fn quick_hide(app: AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

/// "Ask about the screen": steps the box out of the way, takes a screenshot
/// of the screen the mouse is on, and brings the box back. The picture is a
/// hidden attachment (not shown in the studio).
#[tauri::command]
pub async fn quick_screenshot(app: AppHandle, state: AppStateRef<'_>) -> Result<crate::media::MediaItem, String> {
    state.cipher()?;
    let w = app.get_webview_window(LABEL);
    if let Some(w) = &w {
        let _ = w.hide();
    }
    // Let Windows redraw what was behind the box.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let shot = {
        let state = state.inner().clone();
        tokio::task::spawn_blocking(move || crate::media::screenshot(&state, None)).await.map_err(|e| e.to_string())?
    };
    if let Some(w) = &w {
        let _ = w.show();
        let _ = w.set_focus();
    }
    shot
}

/// Opens the main window, on a chat if given.
#[tauri::command]
pub fn quick_open_in_app(app: AppHandle, state: AppStateRef, chat_id: Option<String>) -> Result<(), String> {
    state.cipher()?;
    quick_hide(app.clone());
    show_main(&app);
    if let Some(id) = chat_id {
        app.emit_to("main", "quick:open-chat", json!({ "chat_id": id })).ok();
    }
    Ok(())
}

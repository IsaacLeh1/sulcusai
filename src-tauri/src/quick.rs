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
/// The settings: the hotkey the person picked, and the model its chats use.
const HOTKEY_KEY: &str = "quick_hotkey";
const MODEL_KEY: &str = "quick_model";

/// The hotkey: the one picked, else Ctrl+Alt+Space.
fn hotkey(app: &AppHandle) -> String {
    let state = app.state::<Arc<AppState>>();
    let picked = crate::db::get::<String>(&state.db.lock().unwrap(), HOTKEY_KEY);
    picked.filter(|h| !h.trim().is_empty()).unwrap_or_else(|| HOTKEY.into())
}

/// "ctrl+alt+KeyK" as people write it: "Ctrl+Alt+K".
pub fn shown(hotkey: &str) -> String {
    hotkey
        .split('+')
        .map(|p| {
            let p = p.trim();
            let p = p.strip_prefix("Key").or_else(|| p.strip_prefix("Digit")).unwrap_or(p);
            match p.to_lowercase().as_str() {
                "ctrl" | "control" => "Ctrl".to_string(),
                "alt" | "option" => "Alt".to_string(),
                "shift" => "Shift".to_string(),
                "super" | "cmd" | "command" | "meta" => "Win".to_string(),
                _ => {
                    let mut c = p.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

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
    let key = hotkey(app);
    if on {
        if !gs.is_registered(key.as_str()) {
            if let Err(e) = gs.register(key.as_str()) {
                eprintln!("quick ask hotkey: {e}");
                app.emit("quick:hotkey_taken", json!({ "hotkey": shown(&key) })).ok();
            }
        }
        if app.tray_by_id(TRAY).is_none() {
            if let Err(e) = tray(app) {
                eprintln!("tray: {e}");
            }
        }
    } else {
        let _ = gs.unregister(key.as_str());
        let _ = app.remove_tray_by_id(TRAY);
        if let Some(w) = app.get_webview_window(LABEL) {
            let _ = w.close();
        }
    }
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let ask = MenuItem::with_id(app, "quick", format!("Quick ask   {}", shown(&hotkey(app))), true, None::<&str>)?;
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
        // Hide to the tray icon when there is one (it's there while quick ask
        // is on). Asked of the tray, not the database: this runs on the
        // window's own thread, which must never wait on a lock.
        if window.app_handle().tray_by_id(TRAY).is_some() {
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

#[tauri::command(async)]
pub fn quick_clipboard() -> Option<String> {
    clipboard_text().map(|t| t.trim().chars().take(20_000).collect::<String>()).filter(|t| !t.is_empty())
}

/// The box starts small and grows to fit an answer.
#[tauri::command(async)]
pub fn quick_resize(app: AppHandle, height: f64) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.set_size(tauri::LogicalSize::new(680.0, height.clamp(60.0, 640.0)));
    }
}

#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn quick_open_in_app(app: AppHandle, state: AppStateRef, chat_id: Option<String>) -> Result<(), String> {
    state.cipher()?;
    quick_hide(app.clone());
    show_main(&app);
    if let Some(id) = chat_id {
        app.emit_to("main", "quick:open-chat", json!({ "chat_id": id })).ok();
    }
    Ok(())
}

#[derive(serde::Serialize)]
pub struct QuickSettings {
    hotkey: String,
    /// As people write it ("Ctrl+Alt+Space").
    shown: String,
    /// The model its chats use (None: the default model).
    model: Option<String>,
}

#[tauri::command(async)]
pub fn quick_settings(app: AppHandle, state: AppStateRef) -> QuickSettings {
    let key = hotkey(&app);
    QuickSettings { shown: shown(&key), hotkey: key, model: crate::db::get(&state.db.lock().unwrap(), MODEL_KEY) }
}

/// A new hotkey ("ctrl+alt+KeyK", as the settings page records it): it needs
/// Ctrl, Alt, Shift or Win with a key, and one no other program holds; the
/// old one stays when it can't be had.
#[tauri::command(async)]
pub fn set_quick_hotkey(app: AppHandle, state: AppStateRef, hotkey: String) -> Result<QuickSettings, String> {
    let wanted: tauri_plugin_global_shortcut::Shortcut = hotkey.parse().map_err(|_| "That isn't a key combination SulcusAI can use. Try one like Ctrl+Alt+Space.".to_string())?;
    if wanted.mods.is_empty() {
        return Err("Use Ctrl, Alt, Shift or Win with a key, so typing that key alone doesn't open it.".into());
    }
    let old = self::hotkey(&app);
    if enabled(&app) {
        let gs = app.global_shortcut();
        let _ = gs.unregister(old.as_str());
        if gs.register(wanted).is_err() {
            let _ = gs.register(old.as_str());
            return Err(format!("Another program already uses {}. Pick another combination.", shown(&hotkey)));
        }
    }
    crate::db::set(&state.db.lock().unwrap(), HOTKEY_KEY, &hotkey)?;
    // The tray menu shows it.
    if app.tray_by_id(TRAY).is_some() {
        let _ = app.remove_tray_by_id(TRAY);
        let _ = tray(&app);
    }
    state.log("settings", &format!("Quick ask now opens with {}", shown(&hotkey)));
    Ok(quick_settings(app, state))
}

/// The model Quick ask's chats use (None: the default model).
#[tauri::command(async)]
pub fn set_quick_model(state: AppStateRef, model_id: Option<String>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    match model_id.filter(|m| !m.is_empty()) {
        Some(m) => crate::db::set(&conn, MODEL_KEY, &m),
        None => {
            conn.execute("DELETE FROM settings WHERE key = ?1", [MODEL_KEY]).map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

/// A new chat for the box, on Quick ask's model when one is picked (and
/// still installed).
#[tauri::command(async)]
pub fn quick_chat(state: AppStateRef) -> Result<crate::db::Chat, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let model = crate::db::get::<String>(&conn, MODEL_KEY).filter(|m| crate::db::installed_model(&conn, m).is_some() || m.starts_with("cloud:"));
    crate::db::create_chat_in(&conn, &c, model, None, false)
}

// SPDX-License-Identifier: AGPL-3.0-only
//! The app's place on the desktop: the unread mark on the taskbar button,
//! progress on the taskbar, starting with Windows and staying in the tray.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Manager};

static KEEP_IN_TRAY: AtomicBool = AtomicBool::new(false);

/// Whether closing the window leaves the app in the tray even with Quick ask off.
pub fn keep_in_tray() -> bool {
    KEEP_IN_TRAY.load(Ordering::Relaxed)
}

#[tauri::command]
pub fn desktop_keep_in_tray(app: AppHandle, on: bool) {
    KEEP_IN_TRAY.store(on, Ordering::Relaxed);
    crate::quick::show_paused(&app, crate::quiet::paused());
}

/// A red dot drawn on the fly: Windows overlays it on the taskbar button.
fn dot() -> tauri::image::Image<'static> {
    const N: u32 = 16;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
            let d = (dx * dx + dy * dy).sqrt();
            let a = if d <= 6.5 { 255 } else if d <= 7.5 { ((7.5 - d) * 255.0) as u8 } else { 0 };
            rgba.extend_from_slice(&[0xe1, 0x1d, 0x48, a]);
        }
    }
    tauri::image::Image::new_owned(rgba, N, N)
}

#[tauri::command]
pub fn desktop_set_badge(app: AppHandle, count: u32) {
    let Some(w) = app.get_webview_window("main") else { return };
    #[cfg(windows)]
    let _ = w.set_overlay_icon(if count > 0 { Some(dot()) } else { None });
    #[cfg(not(windows))]
    {
        let _ = dot;
        let _ = w.set_badge_count(if count > 0 { Some(count as i64) } else { None });
    }
}

/// Progress on the taskbar button for the longest-running job; None clears it.
#[tauri::command]
pub fn desktop_set_progress(app: AppHandle, fraction: Option<f64>) {
    let Some(w) = app.get_webview_window("main") else { return };
    let state = match fraction {
        Some(f) => ProgressBarState { status: Some(ProgressBarStatus::Normal), progress: Some((f.clamp(0.0, 1.0) * 100.0) as u64) },
        None => ProgressBarState { status: Some(ProgressBarStatus::None), progress: None },
    };
    let _ = w.set_progress_bar(state);
}

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_NAME: &str = "SulcusAI";

#[cfg(windows)]
#[tauri::command]
pub fn desktop_start_with_windows(on: bool) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::*;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Started this way it waits in the tray, the same as a start for scheduled tasks.
    let line = format!("\"{}\" {}", exe.display(), crate::pause::FOR_TASKS);
    // SAFETY: plain registry calls on the current user's hive.
    unsafe {
        if on {
            let value: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
            let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
            RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_NAME), REG_SZ.0, Some(bytes.as_ptr().cast()), bytes.len() as u32)
                .ok()
                .map_err(|e| format!("Couldn't set SulcusAI to start with Windows: {e}"))
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_NAME));
            Ok(())
        }
    }
}

#[cfg(windows)]
#[tauri::command]
pub fn desktop_starts_with_windows() -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::*;
    // SAFETY: a read of one value's existence; no buffer is passed.
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_NAME), RRF_RT_REG_SZ, None, None, None).is_ok() }
}

#[cfg(not(windows))]
#[tauri::command]
pub fn desktop_start_with_windows(_on: bool) -> Result<(), String> {
    Err("Starting with the computer is set in the system's own settings here.".into())
}

#[cfg(not(windows))]
#[tauri::command]
pub fn desktop_starts_with_windows() -> bool {
    false
}

// ---------- the jump list's chats ----------

static RECENT: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

pub fn recent_chats() -> Vec<(String, String)> {
    RECENT.lock().map(|r| r.clone()).unwrap_or_default()
}

/// The newest chats for the jump list, as (id, title); an empty list hides the group.
#[tauri::command]
pub fn desktop_set_recent_chats(chats: Vec<(String, String)>) {
    let chats: Vec<_> = chats.into_iter().take(6).map(|(id, t)| (id, if t.trim().is_empty() { "New chat".into() } else { t })).collect();
    {
        let Ok(mut r) = RECENT.lock() else { return };
        if *r == chats {
            return;
        }
        *r = chats;
    }
    std::thread::spawn(|| {
        if let Err(e) = crate::instance::refresh_jump_list(crate::pause::jump_list_titles()) {
            crate::applog::line("app", &format!("The jump list's chats couldn't be set: {e}"));
        }
    });
}

fn handed_file() -> std::path::PathBuf {
    std::env::temp_dir().join("sulcusai-open-chat.txt")
}

fn chat_arg(args: &[String]) -> Option<String> {
    let i = args.iter().position(|a| a == crate::instance::Ask::OpenChat.arg())?;
    // Ids are made by the app: letters, digits and dashes only.
    args.get(i + 1).filter(|id| !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).cloned()
}

/// A second start from the jump list leaves the chat's id for the running copy.
pub fn hand_over_chat(args: &[String]) {
    if let Some(id) = chat_arg(args) {
        let _ = std::fs::write(handed_file(), id);
    }
}

pub fn take_handed_chat() -> Option<String> {
    let id = std::fs::read_to_string(handed_file()).ok()?;
    let _ = std::fs::remove_file(handed_file());
    chat_arg(&["".into(), crate::instance::Ask::OpenChat.arg().into(), id.trim().into()])
}

static START_ASK_TAKEN: AtomicBool = AtomicBool::new(false);

/// What this start was asked for from the jump list when no copy was running yet:
/// {"new_chat": true} or {"chat": id}. Answered once.
#[tauri::command]
pub fn desktop_start_ask() -> serde_json::Value {
    if START_ASK_TAKEN.swap(true, Ordering::Relaxed) {
        return serde_json::Value::Null;
    }
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == crate::instance::Ask::NewChat.arg()) {
        return serde_json::json!({ "new_chat": true });
    }
    match chat_arg(&args) {
        Some(id) => serde_json::json!({ "chat": id }),
        None => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_ids_are_checked() {
        let a = |v: &[&str]| chat_arg(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["x", "--open-chat", "ab-12"]), Some("ab-12".into()));
        assert_eq!(a(&["x", "--open-chat", "a b"]), None);
        assert_eq!(a(&["x", "--open-chat"]), None);
    }
}

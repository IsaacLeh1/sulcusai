// SPDX-License-Identifier: AGPL-3.0-only
//! Parts of the app that can run on their own. A part started on its own
//! (`sulcusai.exe --social`) opens only its window and skips models,
//! engines, mail, media and the agent's background work; the rest starts
//! only when the person opens the full app from it, in the same process, so
//! nothing loads twice. A copy already running takes the ask instead.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{db, AppStateRef};

pub const SOCIAL_ARG: &str = "--social";
const KEY: &str = "parts_on_their_own";
static FULL: AtomicBool = AtomicBool::new(false);

/// This start is the light social app.
pub fn light_start() -> bool {
    std::env::args().skip(1).any(|a| a == SOCIAL_ARG)
}

/// Starts the full app's background work, once.
pub fn start_full(app: &AppHandle) {
    if !FULL.swap(true, Ordering::SeqCst) {
        crate::start_full(app);
    }
}

pub fn full_started() -> bool {
    FULL.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PartView {
    pub id: &'static str,
    pub name: &'static str,
    /// Whether it has a "Run on its own" switch.
    pub can_split: bool,
    pub on: bool,
    /// Why it runs where it does.
    pub why: &'static str,
}

/// Every part that was checked for running on its own, and the answer.
pub fn parts(on: &[String]) -> Vec<PartView> {
    let row = |id, name, can_split, why| PartView { id, name, can_split, on: can_split && on.iter().any(|x| x == id), why };
    vec![
        row("social", "Social (groups, chat, voice, video and screen sharing)", true, "Needs only the People link and its own window; opens the full app only for assistants in groups or AI translation."),
        row("extension", "Browser extension host", false, "Already runs on its own: Chrome or Edge starts a small copy that only passes messages along."),
        row("phone", "Phone link", false, "Stays in the main app: the phone uses the PC's chats, models and files, which are the main app."),
        row("relay", "Relay host", false, "Stays where it is: the relay runs on Cloudflare or your own Oracle account, not on this PC."),
        row("dictation", "Dictation", false, "Stays in the main app: it uses the speech model on the graphics card, which a second program would load a second time."),
    ]
}

fn split_ids(state: &crate::AppState) -> Vec<String> {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

#[tauri::command]
pub fn parts_view(state: AppStateRef) -> Vec<PartView> {
    parts(&split_ids(&state))
}

/// Turns "Run on its own" on or off for a part: its Start menu shortcut is
/// made or removed to match.
#[tauri::command]
pub fn parts_set(state: AppStateRef, id: String, on: bool) -> Result<Vec<PartView>, String> {
    if !parts(&[]).iter().any(|p| p.id == id && p.can_split) {
        return Err("That part runs inside the main app.".into());
    }
    let mut ids = split_ids(&state);
    ids.retain(|x| *x != id);
    if on {
        ids.push(id.clone());
    }
    shortcut(on)?;
    db::set(&state.db.lock().unwrap(), KEY, &ids)?;
    crate::db::log_action(&state.db.lock().unwrap(), "app", if on { "Social runs on its own: Start menu shortcut added" } else { "Social is back in the main app: shortcut removed" });
    Ok(parts(&ids))
}

/// Started as the light social app: brings in everything else and shows the
/// main window.
#[tauri::command]
pub async fn parts_open_full(app: AppHandle) -> Result<(), String> {
    start_full(&app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub fn parts_light() -> bool {
    light_start() && !full_started()
}

#[cfg(windows)]
fn shortcut_path() -> Result<std::path::PathBuf, String> {
    let roaming = std::env::var_os("APPDATA").ok_or("The Start menu folder couldn't be found.")?;
    Ok(std::path::Path::new(&roaming).join("Microsoft\\Windows\\Start Menu\\Programs\\SulcusAI Social.lnk"))
}

/// The light app's own taskbar identity, so it pins apart from the main app.
pub const SOCIAL_APP_ID: &str = "app.sulcusai.social";

#[cfg(windows)]
pub fn set_process_app_id() {
    // SAFETY: sets a string the shell copies.
    unsafe {
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&windows::core::HSTRING::from(SOCIAL_APP_ID));
    }
}

#[cfg(not(windows))]
pub fn set_process_app_id() {}

#[cfg(windows)]
fn shortcut(on: bool) -> Result<(), String> {
    let path = shortcut_path()?;
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::thread::spawn(move || -> Result<(), String> {
        use windows::core::{Interface, HSTRING, PROPVARIANT};
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
        use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PROPERTYKEY};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        // PKEY_AppUserModel_ID: the shortcut and the window share it, so the
        // taskbar treats the light app as an app of its own.
        const APP_ID: PROPERTYKEY = PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 5 };
        // SAFETY: COM objects made, used and released on this thread.
        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let r = (|| -> windows::core::Result<()> {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(&HSTRING::from(exe.as_os_str()))?;
                link.SetArguments(&HSTRING::from(SOCIAL_ARG))?;
                link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
                link.SetDescription(&HSTRING::from("SulcusAI Social: groups, chat and calls"))?;
                let store: IPropertyStore = link.cast()?;
                store.SetValue(&APP_ID, &PROPVARIANT::from(SOCIAL_APP_ID))?;
                store.Commit()?;
                link.cast::<IPersistFile>()?.Save(&HSTRING::from(path.as_os_str()), true)
            })();
            if com {
                CoUninitialize();
            }
            r.map_err(|e| format!("The Start menu shortcut couldn't be made: {}", e.message()))
        }
    })
    .join()
    .map_err(|_| "The Start menu shortcut couldn't be made.".to_string())?
}

#[cfg(not(windows))]
fn shortcut(_on: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_social_splits_and_every_part_says_why() {
        let all = parts(&["social".into(), "phone".into()]);
        assert_eq!(all.iter().filter(|p| p.can_split).map(|p| p.id).collect::<Vec<_>>(), vec!["social"]);
        assert!(all.iter().find(|p| p.id == "social").unwrap().on);
        // A part that can't split never reads as on.
        assert!(!all.iter().find(|p| p.id == "phone").unwrap().on);
        assert!(all.iter().all(|p| !p.why.is_empty()));
        assert_eq!(crate::instance::asked(&["x".into(), SOCIAL_ARG.into()]), Some(crate::instance::Ask::Social));
    }
}

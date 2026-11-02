// SPDX-License-Identifier: AGPL-3.0-only
//! Chats protected with the app lock's PIN: shut until the PIN or Windows
//! Hello opens them, and shut again when the window leaves them or the app
//! locks. While shut, their titles and words stay in the core.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use tauri::WebviewWindow;

use crate::{db, hello, AppState, AppStateRef};

/// The error a shut chat gives; the window shows the PIN box on it.
pub const SHUT: &str = "This chat is protected with your PIN.";

fn open_now() -> &'static Mutex<HashSet<String>> {
    static OPEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    OPEN.get_or_init(Default::default)
}

pub fn shut_all() {
    open_now().lock().unwrap().clear();
}

/// Whether a chat's title and words may be shown or read now.
pub fn readable(chat: &db::Chat) -> bool {
    !chat.protected || open_now().lock().unwrap().contains(&chat.id)
}

/// Refuses a shut chat.
pub fn check(state: &AppState, chat_id: &str) -> Result<(), String> {
    let c = state.cipher()?;
    match db::chat(&state.db.lock().unwrap(), &c, chat_id) {
        Some(chat) if !readable(&chat) => Err(SHUT.into()),
        _ => Ok(()),
    }
}

/// A shut chat with its title taken out.
pub fn masked(mut chat: db::Chat) -> db::Chat {
    if !readable(&chat) {
        chat.title = String::new();
        chat.shut = true;
    }
    chat
}

#[tauri::command(async)]
pub fn protect_chat(state: AppStateRef, chat_id: String) -> Result<(), String> {
    let c = state.cipher()?;
    if !state.vault.lock().unwrap().lock_enabled() {
        return Err("Set a PIN under Settings › Security first: a protected chat opens with it.".into());
    }
    let conn = state.db.lock().unwrap();
    let chat = db::chat(&conn, &c, &chat_id).ok_or("That chat is no longer here.")?;
    if chat.incognito {
        return Err("An incognito chat is deleted when you leave it, so there's nothing to protect.".into());
    }
    db::set_chat_protected(&conn, &chat_id, true)?;
    db::log_action(&conn, "security", "Protected a chat with the PIN");
    drop(conn);
    // It stays open while the window shows it; leaving it shuts it.
    open_now().lock().unwrap().insert(chat_id);
    Ok(())
}

/// Opens a protected chat with the PIN, or with `unprotect` takes the
/// protection off.
#[tauri::command]
pub async fn open_protected_chat(state: AppStateRef<'_>, chat_id: String, pin: String, unprotect: bool) -> Result<(), String> {
    let wait = state.vault.lock().unwrap().penalty();
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    let checked = crate::security::blocking(state.inner(), move |s| s.vault.lock().unwrap().check_pin(&pin)).await;
    if checked.is_err() {
        state.log("security", "Wrong PIN entered for a protected chat");
    }
    checked?;
    opened(&state, chat_id, unprotect)
}

#[tauri::command]
pub async fn open_protected_chat_hello(window: WebviewWindow, state: AppStateRef<'_>, chat_id: String, unprotect: bool) -> Result<(), String> {
    if !state.vault.lock().unwrap().hello_enabled() {
        return Err("Windows Hello isn't set up for SulcusAI.".into());
    }
    let hwnd = crate::security::hwnd_of(&window)?;
    let ok = tauri::async_runtime::spawn_blocking(move || hello::verify(hwnd, "Open a protected chat"))
        .await
        .map_err(|e| e.to_string())??;
    if !ok {
        return Err("Windows Hello didn't verify you.".into());
    }
    opened(&state, chat_id, unprotect)
}

fn opened(state: &AppState, chat_id: String, unprotect: bool) -> Result<(), String> {
    if unprotect {
        db::set_chat_protected(&state.db.lock().unwrap(), &chat_id, false)?;
        state.log("security", "Took the PIN protection off a chat");
    } else {
        state.log("security", "Opened a protected chat");
    }
    open_now().lock().unwrap().insert(chat_id);
    Ok(())
}

/// The window left the chat: it needs the PIN again next time.
#[tauri::command(async)]
pub fn shut_protected_chat(chat_id: String) {
    open_now().lock().unwrap().remove(&chat_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_protected_chat_is_shut_until_opened() {
        let chat = db::Chat { id: "protected-test".into(), title: "Plans".into(), protected: true, ..Default::default() };
        assert!(!readable(&chat));
        assert_eq!(masked(chat.clone()).title, "");
        assert!(masked(chat.clone()).shut);
        open_now().lock().unwrap().insert(chat.id.clone());
        assert!(readable(&chat));
        assert_eq!(masked(chat.clone()).title, "Plans");
        shut_protected_chat(chat.id.clone());
        assert!(!readable(&chat));
        let plain = db::Chat { id: "plain-test".into(), title: "Hi".into(), ..Default::default() };
        assert!(readable(&plain));
    }
}

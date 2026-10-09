// SPDX-License-Identifier: AGPL-3.0-only
//! App lock, Windows Hello and the action log, as commands for the window.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, WebviewWindow};

use crate::crypto::UnlockWith;
use crate::db::{self, Action};
use crate::{hello, AppState, AppStateRef};

#[derive(Serialize)]
pub struct SecurityStatus {
    locked: bool,
    lock_enabled: bool,
    hello_enabled: bool,
    hello_available: bool,
    auto_lock_minutes: u32,
    keep_working: bool,
}

/// Runs vault work (Argon2 takes a moment) off the async threads.
async fn blocking<T: Send + 'static>(
    state: &Arc<AppState>,
    f: impl FnOnce(&AppState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let state = state.clone();
    tauri::async_runtime::spawn_blocking(move || f(&state)).await.map_err(|e| e.to_string())?
}

fn hwnd_of(window: &WebviewWindow) -> Result<isize, String> {
    #[cfg(windows)]
    {
        Ok(window.hwnd().map_err(|e| e.to_string())?.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Err("Windows Hello is only available on Windows.".into())
    }
}

#[tauri::command]
pub async fn security_status(state: AppStateRef<'_>) -> Result<SecurityStatus, String> {
    let hello_available = tauri::async_runtime::spawn_blocking(hello::available).await.unwrap_or(false);
    // Settings first: the database is never locked while the vault is.
    let auto_lock_minutes = state.settings().auto_lock_minutes;
    let keep_working = keep_working(&state);
    let v = state.vault.lock().unwrap();
    Ok(SecurityStatus {
        locked: v.locked(),
        lock_enabled: v.lock_enabled(),
        hello_enabled: v.hello_enabled(),
        hello_available,
        auto_lock_minutes,
        keep_working,
    })
}

const KEEP_WORKING: &str = "keep_working_locked";

fn keep_working(state: &crate::AppState) -> bool {
    crate::db::get::<bool>(&state.db.lock().unwrap(), KEEP_WORKING).unwrap_or(false)
}

/// Whether locking lets replies, schedules, syncing and reminders carry on.
#[tauri::command]
pub fn set_keep_working(state: AppStateRef, on: bool) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    crate::db::set(&conn, KEEP_WORKING, &on)?;
    crate::db::log_action(&conn, "security", if on { "Locking now lets work in progress carry on" } else { "Locking now stops work in progress" });
    Ok(())
}

#[tauri::command]
pub async fn enable_lock(state: AppStateRef<'_>, pin: String) -> Result<String, String> {
    let code = blocking(state.inner(), move |s| s.vault.lock().unwrap().enable_lock(&pin)).await?;
    state.log("security", "Turned on app lock and created a recovery code");
    Ok(code)
}

#[tauri::command]
pub async fn change_pin(state: AppStateRef<'_>, old_pin: String, new_pin: String) -> Result<(), String> {
    blocking(state.inner(), move |s| s.vault.lock().unwrap().change_pin(&old_pin, &new_pin)).await?;
    state.log("security", "Changed the app lock PIN");
    Ok(())
}

/// After a recovery-code unlock: new PIN, new recovery code.
#[tauri::command]
pub async fn reset_pin(state: AppStateRef<'_>, new_pin: String) -> Result<String, String> {
    let code = blocking(state.inner(), move |s| s.vault.lock().unwrap().reset_pin(&new_pin)).await?;
    state.log("security", "Set a new PIN and replaced the used recovery code");
    Ok(code)
}

#[tauri::command]
pub async fn disable_lock(state: AppStateRef<'_>, pin: String) -> Result<(), String> {
    blocking(state.inner(), move |s| s.vault.lock().unwrap().disable_lock(&pin)).await?;
    state.log("security", "Turned off app lock");
    Ok(())
}

#[tauri::command]
pub async fn unlock(state: AppStateRef<'_>, method: String, secret: String) -> Result<(), String> {
    let how = match method.as_str() {
        "pin" => UnlockWith::Pin,
        "recovery" => UnlockWith::RecoveryCode,
        _ => return Err("Unknown unlock method.".into()),
    };
    let wait = state.vault.lock().unwrap().penalty();
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    let result = blocking(state.inner(), move |s| s.vault.lock().unwrap().unlock(how, &secret)).await;
    match &result {
        Ok(()) => {
            let label = if how == UnlockWith::Pin { "PIN" } else { "recovery code" };
            state.log("security", &format!("Unlocked with the {label}"));
            if let Ok(c) = state.cipher() {
                state.migrate_plaintext(&c);
            }
        }
        Err(_) => state.log("security", "Wrong PIN or recovery code entered"),
    }
    result
}

#[tauri::command]
pub async fn unlock_with_hello(window: WebviewWindow, state: AppStateRef<'_>) -> Result<(), String> {
    if !state.vault.lock().unwrap().hello_enabled() {
        return Err("Windows Hello isn't set up for SulcusAI.".into());
    }
    let hwnd = hwnd_of(&window)?;
    let ok = tauri::async_runtime::spawn_blocking(move || hello::verify(hwnd, "Unlock SulcusAI"))
        .await
        .map_err(|e| e.to_string())??;
    if !ok {
        return Err("Windows Hello didn't verify you.".into());
    }
    state.vault.lock().unwrap().unlock_after_hello()?;
    state.log("security", "Unlocked with Windows Hello");
    Ok(())
}

#[tauri::command]
pub fn lock_now(app: AppHandle, state: AppStateRef) -> Result<(), String> {
    // Read before taking the vault: the database is never locked while the vault is.
    let keep_working = keep_working(&state);
    let mut v = state.vault.lock().unwrap();
    if !v.lock_enabled() {
        return Err("Turn on app lock in Settings first.".into());
    }
    if keep_working {
        v.lock_screen();
    } else {
        v.lock();
    }
    drop(v);
    if !keep_working {
        // Stop anything still writing replies.
        for flag in state.generations.lock().unwrap().values() {
            flag.store(true, Ordering::Relaxed);
        }
    }
    state.contexts.lock().unwrap().clear();
    state.log("security", if keep_working { "Locked (work in progress keeps going)" } else { "Locked" });
    app.emit("security:locked", ()).ok();
    Ok(())
}

#[tauri::command]
pub async fn set_hello(window: WebviewWindow, state: AppStateRef<'_>, enabled: bool) -> Result<(), String> {
    state.cipher()?;
    if enabled {
        // Make sure Hello works for this person before relying on it.
        let hwnd = hwnd_of(&window)?;
        let ok = tauri::async_runtime::spawn_blocking(move || hello::verify(hwnd, "Use Windows Hello to unlock SulcusAI"))
            .await
            .map_err(|e| e.to_string())??;
        if !ok {
            return Err("Windows Hello didn't verify you, so it wasn't turned on.".into());
        }
    }
    state.vault.lock().unwrap().set_hello(enabled)?;
    state.log("security", if enabled { "Turned on Windows Hello unlock" } else { "Turned off Windows Hello unlock" });
    Ok(())
}

#[tauri::command]
pub fn set_auto_lock(state: AppStateRef, minutes: u32) -> Result<(), String> {
    state.cipher()?;
    db::update_settings(&state.db.lock().unwrap(), |s| s.auto_lock_minutes = minutes.min(24 * 60))?;
    Ok(())
}

#[tauri::command]
pub fn list_actions(state: AppStateRef, limit: Option<u32>) -> Result<Vec<Action>, String> {
    let c = state.cipher()?;
    Ok(db::actions(&state.db.lock().unwrap(), &c, limit.unwrap_or(500).min(5000)))
}

#[tauri::command]
pub fn clear_actions(state: AppStateRef) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    db::clear_actions(&conn)?;
    db::log_action(&conn, "privacy", "Cleared the activity log");
    Ok(())
}

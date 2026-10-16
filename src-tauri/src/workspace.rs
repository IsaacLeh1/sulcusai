// SPDX-License-Identifier: AGPL-3.0-only
//! Commands for agent work: shared folders, run mode, approvals and undo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::Serialize;
use tauri::Manager;

use crate::agent::{Decision, PendingApproval};
use crate::features::{self, Feature};
use crate::{checkpoint, db, sandbox, AppState, AppStateRef};

/// The user's Desktop (where Windows really keeps it, OneDrive included).
pub fn desktop(state: &AppState) -> Option<PathBuf> {
    state.app.get().and_then(|a| a.path().desktop_dir().ok()).or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Desktop")))
}

/// Shares a folder with the assistant (making it first when `create`, inside
/// a folder that exists): refuses what `sandbox::refuse_reason` refuses and
/// names that clash. Returns it as shared.
pub fn share_folder(state: &AppState, path: &Path, allow_system: bool, create: bool) -> Result<String, String> {
    let made = create && !path.exists();
    if made {
        std::fs::create_dir(path).map_err(|e| format!("Couldn't make that folder ({e})."))?;
    }
    if let Some(reason) = sandbox::refuse_reason(path, &state.paths.data, allow_system) {
        if made {
            // Still empty: it was only just made.
            std::fs::remove_dir(path).ok();
        }
        return Err(reason.into());
    }
    let canonical = dunce::canonicalize(path).map_err(|e| e.to_string())?;
    let shown = canonical.display().to_string();
    let conn = state.db.lock().unwrap();
    if db::folders(&conn).iter().any(|f| f.eq_ignore_ascii_case(&shown)) {
        return Ok(shown);
    }
    let name = canonical.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let clash = db::folders(&conn).into_iter().any(|f| {
        Path::new(&f).file_name().map(|n| n.to_string_lossy().to_lowercase()) == name && !f.eq_ignore_ascii_case(&shown)
    });
    if clash {
        return Err("Another shared folder already has that name. Rename one of them so the assistant can tell them apart.".into());
    }
    db::add_folder(&conn, &shown)?;
    db::log_action(&conn, "privacy", "Shared a folder with the assistant");
    Ok(shown)
}

/// Folders the assistant asked for (by the request's id), changed if the
/// user picked another one before allowing it.
static FOLDER_ASKS: LazyLock<Mutex<HashMap<String, PathBuf>>> = LazyLock::new(Default::default);

pub fn offer_folder(call_id: &str, path: &Path) {
    FOLDER_ASKS.lock().unwrap().insert(call_id.to_string(), path.to_path_buf());
}

pub fn chosen_folder(call_id: &str) -> Option<PathBuf> {
    FOLDER_ASKS.lock().unwrap().remove(call_id)
}

/// The user picked another folder for the assistant's request.
#[tauri::command(async)]
pub fn choose_folder(state: AppStateRef, call_id: String, path: String) -> Result<(), String> {
    state.cipher()?;
    let p = PathBuf::from(path.trim());
    if !p.is_absolute() {
        return Err("Pick a folder.".into());
    }
    if let Some(reason) = sandbox::refuse_reason(&p, &state.paths.data, true) {
        return Err(reason.into());
    }
    FOLDER_ASKS.lock().unwrap().insert(call_id, p);
    Ok(())
}

#[derive(Serialize)]
pub struct DesktopAccess {
    on: bool,
    path: Option<String>,
}

fn desktop_access_of(state: &AppState) -> DesktopAccess {
    let path = desktop(state).and_then(|d| dunce::canonicalize(d).ok()).map(|d| d.display().to_string());
    let on = path.as_ref().is_some_and(|p| db::folders(&state.db.lock().unwrap()).iter().any(|f| f.eq_ignore_ascii_case(p)));
    DesktopAccess { on, path }
}

/// Whether the assistant may use anything on the Desktop.
#[tauri::command(async)]
pub fn desktop_access(state: AppStateRef) -> DesktopAccess {
    desktop_access_of(&state)
}

/// Lets the assistant use anything on the Desktop (in every chat), or not.
/// Turning it on turns on Files and coding too.
#[tauri::command(async)]
pub fn set_desktop_access(state: AppStateRef, on: bool) -> Result<DesktopAccess, String> {
    state.cipher()?;
    let d = desktop(&state).ok_or("Couldn't find your Desktop.")?;
    if on {
        if !features::is_on(&state.db.lock().unwrap(), Feature::Files) {
            features::set(&state.db.lock().unwrap(), Feature::Files, true)?;
            if let Some(app) = state.app.get() {
                use tauri::Emitter;
                app.emit("features:changed", serde_json::json!({})).ok();
            }
        }
        share_folder(&state, &d, false, false)?;
        state.log("privacy", "Let the assistant use anything on the Desktop");
    } else {
        let shown = dunce::canonicalize(&d).map(|p| p.display().to_string()).unwrap_or_default();
        db::remove_folder(&state.db.lock().unwrap(), &shown)?;
        state.log("privacy", "Stopped letting the assistant use the Desktop");
    }
    Ok(desktop_access_of(&state))
}

#[derive(Serialize)]
pub struct Folder {
    path: String,
    name: String,
    exists: bool,
}

#[tauri::command(async)]
pub fn list_folders(state: AppStateRef) -> Result<Vec<Folder>, String> {
    state.cipher()?;
    Ok(db::folders(&state.db.lock().unwrap())
        .into_iter()
        .map(|p| {
            let path = Path::new(&p);
            Folder {
                name: path.file_name().map_or_else(|| p.clone(), |n| n.to_string_lossy().to_string()),
                exists: path.is_dir(),
                path: p,
            }
        })
        .collect())
}

/// The warning to confirm before sharing this folder (one inside Windows,
/// Program Files or ProgramData), if it needs one.
#[tauri::command(async)]
pub fn folder_warning(state: AppStateRef, path: String) -> Option<String> {
    let p = Path::new(&path);
    (sandbox::refuse_reason(p, &state.paths.data, true).is_none() && sandbox::inside_system(p)).then(|| sandbox::SYSTEM_WARNING.to_string())
}

/// Shares a folder; `allow_system` once the person confirmed the warning.
#[tauri::command(async)]
pub fn add_folder(state: AppStateRef, path: String, allow_system: Option<bool>) -> Result<(), String> {
    crate::features::require(&state, crate::features::Feature::Files)?;
    state.cipher()?;
    share_folder(&state, Path::new(&path), allow_system.unwrap_or(false), false).map(|_| ())
}

#[tauri::command(async)]
pub fn remove_folder(state: AppStateRef, path: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    db::remove_folder(&conn, &path)?;
    db::log_action(&conn, "privacy", "Stopped sharing a folder with the assistant");
    Ok(())
}

#[tauri::command(async)]
pub fn set_chat_mode(state: AppStateRef, chat_id: String, mode: String) -> Result<(), String> {
    state.cipher()?;
    db::set_chat_mode(&state.db.lock().unwrap(), &chat_id, &mode)?;
    if mode == "bypass" {
        state.log("privacy", "Turned on Bypass mode for a chat (changes run without asking)");
    }
    Ok(())
}

#[tauri::command(async)]
pub fn answer_approval(state: AppStateRef, call_id: String, decision: String) -> Result<(), String> {
    state.cipher()?;
    let d = Decision::parse(&decision).ok_or("Unknown answer.")?;
    if !state.approvals.answer(&call_id, d) {
        return Err("That request is no longer waiting.".into());
    }
    Ok(())
}

#[tauri::command(async)]
pub fn pending_approvals(state: AppStateRef, chat_id: String) -> Result<Vec<PendingApproval>, String> {
    state.cipher()?;
    Ok(state.approvals.pending(&chat_id))
}

#[tauri::command(async)]
pub fn undoable_turns(state: AppStateRef, chat_id: String) -> Result<Vec<String>, String> {
    state.cipher()?;
    Ok(db::undoable_turns(&state.db.lock().unwrap(), &chat_id))
}

/// Puts back every file the assistant changed in one turn. Returns notes
/// about anything that couldn't be restored.
#[tauri::command(async)]
pub fn undo_turn(state: AppStateRef, chat_id: String, turn_id: String) -> Result<Vec<String>, String> {
    let c = state.cipher()?;
    if state.generations.lock().unwrap().contains_key(&chat_id) {
        return Err("Wait for the assistant to finish (or press Stop) before undoing.".into());
    }
    let conn = state.db.lock().unwrap();
    let notes = checkpoint::undo_turn(&conn, &c, &turn_id);
    db::log_action(&conn, "agent", "Undid the assistant's file changes from one reply");
    Ok(notes)
}

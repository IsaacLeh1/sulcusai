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

/// Shares a folder with the assistant, in one chat (`chat`) or every chat
/// (None), making it first when `create` (inside a folder that exists):
/// refuses what `sandbox::refuse_reason` refuses and names that clash with
/// a folder the same chats can use. Returns it as shared.
pub fn share_folder(state: &AppState, path: &Path, allow_system: bool, create: bool, chat: Option<&str>) -> Result<String, String> {
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
    // What the same chats can use already: every chat's folders, and this
    // chat's own (for every chat, any chat's own could clash).
    let mut seen = db::folders(&conn);
    match chat {
        Some(id) => seen.extend(db::chat_shared_folders(&conn, id)),
        None => seen.extend(all_chat_folders(&conn)),
    }
    let everywhere = db::folders(&conn).iter().any(|f| f.eq_ignore_ascii_case(&shown));
    if everywhere || chat.is_some_and(|id| db::chat_shared_folders(&conn, id).iter().any(|f| f.eq_ignore_ascii_case(&shown))) {
        return Ok(shown);
    }
    let name = canonical.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let clash = seen.into_iter().any(|f| Path::new(&f).file_name().map(|n| n.to_string_lossy().to_lowercase()) == name && !f.eq_ignore_ascii_case(&shown));
    if clash {
        return Err("Another shared folder already has that name. Rename one of them so the assistant can tell them apart.".into());
    }
    match chat {
        Some(id) => {
            db::add_chat_folder(&conn, id, &shown)?;
            db::log_action(&conn, "privacy", "Shared a folder with the assistant in one chat");
        }
        None => {
            db::add_folder(&conn, &shown)?;
            db::log_action(&conn, "privacy", "Shared a folder with the assistant in every chat");
        }
    }
    Ok(shown)
}

/// Every folder shared in one chat or another.
fn all_chat_folders(conn: &rusqlite::Connection) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT path FROM chat_shared_folders") else { return Vec::new() };
    stmt.query_map([], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Folders the assistant asked for that the person let every chat use (by
/// the request's id); the rest are shared in the asking chat only.
static FOLDER_EVERYWHERE: LazyLock<Mutex<std::collections::HashSet<String>>> = LazyLock::new(Default::default);

/// Whether the folder the assistant asked for goes to every chat.
pub fn shared_everywhere(call_id: &str) -> bool {
    FOLDER_EVERYWHERE.lock().unwrap().remove(call_id)
}

/// The person ticked (or unticked) "every chat" on the assistant's folder request.
#[tauri::command(async)]
pub fn folder_for_every_chat(state: AppStateRef, call_id: String, on: bool) -> Result<(), String> {
    state.cipher()?;
    let mut set = FOLDER_EVERYWHERE.lock().unwrap();
    if on {
        set.insert(call_id);
    } else {
        set.remove(&call_id);
    }
    Ok(())
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

/// The folder a request would share now (kept for the request).
pub fn asked_folder(call_id: &str) -> Option<PathBuf> {
    FOLDER_ASKS.lock().unwrap().get(call_id).cloned()
}

/// Whether a request can be allowed from the phone: not one for a folder
/// inside Windows, Program Files or ProgramData, which needs the
/// confirmation the PC shows.
pub fn phone_may_allow(call_id: &str) -> Result<(), String> {
    match asked_folder(call_id) {
        Some(p) if sandbox::inside_system(&p) => Err("That folder is inside a Windows system folder. Allow it on the PC, where you can confirm it.".into()),
        _ => Ok(()),
    }
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
        share_folder(&state, &d, false, false, None)?;
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
    /// Who can use it: "chat" (this chat only), "project" (the chat's
    /// project's) or "all" (every chat).
    scope: &'static str,
}

/// The folders a chat can use (its own, its project's, then every chat's),
/// or with no chat, the ones every chat can use.
#[tauri::command(async)]
pub fn list_folders(state: AppStateRef, chat_id: Option<String>) -> Result<Vec<Folder>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut list: Vec<(String, &'static str)> = Vec::new();
    if let Some(chat) = chat_id.as_deref().and_then(|id| db::chat(&conn, &c, id)) {
        list.extend(db::chat_shared_folders(&conn, &chat.id).into_iter().map(|p| (p, "chat")));
        if let Some(pid) = &chat.project_id {
            list.extend(crate::projects::folders(&conn, pid).into_iter().map(|p| (p, "project")));
        }
    }
    for p in db::folders(&conn) {
        list.retain(|(q, _)| !q.eq_ignore_ascii_case(&p));
        list.push((p, "all"));
    }
    Ok(list
        .into_iter()
        .map(|(p, scope)| {
            let path = Path::new(&p);
            Folder { name: path.file_name().map_or_else(|| p.clone(), |n| n.to_string_lossy().to_string()), exists: path.is_dir(), path: p, scope }
        })
        .collect())
}

/// Lets every chat use a folder shared in one chat, or makes one every chat
/// had this chat's alone.
#[tauri::command(async)]
pub fn set_folder_every_chat(state: AppStateRef, path: String, chat_id: String, on: bool) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if on {
        db::add_folder(&conn, &path)?;
        db::remove_chat_folder(&conn, &chat_id, &path)?;
        db::log_action(&conn, "privacy", "Let every chat use a shared folder");
    } else {
        db::add_chat_folder(&conn, &chat_id, &path)?;
        db::remove_folder(&conn, &path)?;
        db::log_action(&conn, "privacy", "Kept a shared folder to one chat");
    }
    Ok(())
}

/// The warning to confirm before sharing this folder (one inside Windows,
/// Program Files or ProgramData), if it needs one.
#[tauri::command(async)]
pub fn folder_warning(state: AppStateRef, path: String) -> Option<String> {
    let p = Path::new(&path);
    // One that isn't there yet is made when it's shared: warned about by where it would be.
    let refused = p.exists() && sandbox::refuse_reason(p, &state.paths.data, true).is_some();
    (!refused && sandbox::inside_system(p)).then(|| sandbox::SYSTEM_WARNING.to_string())
}

/// Shares a folder in one chat (`chat_id`) or every chat; `allow_system`
/// once the person confirmed the warning.
#[tauri::command(async)]
pub fn add_folder(state: AppStateRef, path: String, allow_system: Option<bool>, chat_id: Option<String>) -> Result<(), String> {
    crate::features::require(&state, crate::features::Feature::Files)?;
    state.cipher()?;
    share_folder(&state, Path::new(&path), allow_system.unwrap_or(false), false, chat_id.as_deref()).map(|_| ())
}

/// Stops sharing a folder: in this chat when it was shared there, else in every chat.
#[tauri::command(async)]
pub fn remove_folder(state: AppStateRef, path: String, chat_id: Option<String>) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    let in_chat = chat_id.as_deref().is_some_and(|id| db::chat_shared_folders(&conn, id).iter().any(|f| f.eq_ignore_ascii_case(&path)));
    match chat_id.as_deref().filter(|_| in_chat) {
        Some(id) => db::remove_chat_folder(&conn, id, &path)?,
        None => db::remove_folder(&conn, &path)?,
    }
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

/// Every approval waiting for an answer, in all chats (after unlocking, or
/// when the window opens, so none is missed).
#[tauri::command(async)]
pub fn all_pending_approvals(state: AppStateRef) -> Result<Vec<PendingApproval>, String> {
    state.cipher()?;
    Ok(state.approvals.all_waiting())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_folders_cant_be_allowed_from_the_phone() {
        let id = format!("call-{}", uuid::Uuid::new_v4().simple());
        assert!(phone_may_allow(&id).is_ok(), "not a folder request");
        offer_folder(&id, &std::env::temp_dir().join("portfolio-site"));
        assert!(phone_may_allow(&id).is_ok(), "an ordinary folder");
        if let Some(win) = std::env::var_os("WINDIR") {
            // Inside Windows, whether it's there yet or not.
            offer_folder(&id, &PathBuf::from(&win).join("System32"));
            assert!(phone_may_allow(&id).is_err());
            offer_folder(&id, &PathBuf::from(&win).join("sulcus-new-site"));
            assert!(phone_may_allow(&id).is_err());
        }
        assert!(chosen_folder(&id).is_some());
    }
}

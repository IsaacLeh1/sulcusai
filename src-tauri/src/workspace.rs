// SPDX-License-Identifier: AGPL-3.0-only
//! Commands for agent work: shared folders, run mode, approvals and undo.

use std::path::Path;

use serde::Serialize;

use crate::agent::{Decision, PendingApproval};
use crate::{checkpoint, db, sandbox, AppStateRef};

#[derive(Serialize)]
pub struct Folder {
    path: String,
    name: String,
    exists: bool,
}

#[tauri::command]
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

#[tauri::command]
pub fn add_folder(state: AppStateRef, path: String) -> Result<(), String> {
    crate::features::require(&state, crate::features::Feature::Files)?;
    state.cipher()?;
    if let Some(reason) = sandbox::refuse_reason(Path::new(&path), &state.paths.data) {
        return Err(reason.into());
    }
    let canonical = dunce::canonicalize(&path).map_err(|e| e.to_string())?;
    let shown = canonical.display().to_string();
    let conn = state.db.lock().unwrap();
    let name = canonical.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let clash = db::folders(&conn).into_iter().any(|f| {
        Path::new(&f).file_name().map(|n| n.to_string_lossy().to_lowercase()) == name && !f.eq_ignore_ascii_case(&shown)
    });
    if clash {
        return Err("Another shared folder already has that name. Rename one of them so the assistant can tell them apart.".into());
    }
    db::add_folder(&conn, &shown)?;
    db::log_action(&conn, "privacy", "Shared a folder with the assistant");
    Ok(())
}

#[tauri::command]
pub fn remove_folder(state: AppStateRef, path: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    db::remove_folder(&conn, &path)?;
    db::log_action(&conn, "privacy", "Stopped sharing a folder with the assistant");
    Ok(())
}

#[tauri::command]
pub fn set_chat_mode(state: AppStateRef, chat_id: String, mode: String) -> Result<(), String> {
    state.cipher()?;
    db::set_chat_mode(&state.db.lock().unwrap(), &chat_id, &mode)?;
    if mode == "bypass" {
        state.log("privacy", "Turned on Bypass mode for a chat (changes run without asking)");
    }
    Ok(())
}

#[tauri::command]
pub fn answer_approval(state: AppStateRef, call_id: String, decision: String) -> Result<(), String> {
    state.cipher()?;
    let d = Decision::parse(&decision).ok_or("Unknown answer.")?;
    if !state.approvals.answer(&call_id, d) {
        return Err("That request is no longer waiting.".into());
    }
    Ok(())
}

#[tauri::command]
pub fn pending_approvals(state: AppStateRef, chat_id: String) -> Result<Vec<PendingApproval>, String> {
    state.cipher()?;
    Ok(state.approvals.pending(&chat_id))
}

#[tauri::command]
pub fn undoable_turns(state: AppStateRef, chat_id: String) -> Result<Vec<String>, String> {
    state.cipher()?;
    Ok(db::undoable_turns(&state.db.lock().unwrap(), &chat_id))
}

/// Puts back every file the assistant changed in one turn. Returns notes
/// about anything that couldn't be restored.
#[tauri::command]
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

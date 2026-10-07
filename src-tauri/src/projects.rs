// SPDX-License-Identifier: AGPL-3.0-only
//! Projects: workspaces with their own instructions, folders, memories and
//! chats. Plus the memory commands, which projects scope.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::crypto::Cipher;
use crate::db::{self, now_ms, Chat};
use crate::memory::{self, Memory};
use crate::{sandbox, AppStateRef};

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub instructions: String,
    pub folders: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn folders(conn: &Connection, id: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT path FROM project_folders WHERE project_id = ?1 ORDER BY path") else { return Vec::new() };
    stmt.query_map([id], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn list(conn: &Connection, c: &Cipher) -> Vec<Project> {
    let Ok(mut stmt) = conn.prepare("SELECT id, name, instructions, created_at, updated_at FROM projects ORDER BY updated_at DESC") else {
        return Vec::new();
    };
    let rows: Vec<(String, String, String, i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();
    rows.into_iter()
        .map(|(id, name, instructions, created_at, updated_at)| Project {
            folders: folders(conn, &id),
            name: c.decrypt_or(&name, "(unreadable project)"),
            instructions: c.decrypt_or(&instructions, ""),
            id,
            created_at,
            updated_at,
        })
        .collect()
}

pub fn get(conn: &Connection, c: &Cipher, id: &str) -> Option<Project> {
    list(conn, c).into_iter().find(|p| p.id == id)
}

fn exists(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT 1 FROM projects WHERE id = ?1", [id], |_| Ok(())).optional().ok().flatten().is_some()
}

/// Folders a chat may use: the shared ones plus its project's.
pub fn chat_folders(conn: &Connection, chat: &Chat) -> Vec<String> {
    let mut all = db::folders(conn);
    if let Some(pid) = &chat.project_id {
        for f in folders(conn, pid) {
            if !all.iter().any(|a| a.eq_ignore_ascii_case(&f)) {
                all.push(f);
            }
        }
    }
    all
}

/// The project's instructions, for the system prompt.
pub fn prompt_section(p: &Project) -> String {
    let mut s = format!("\n\n## Project: {}\nThis chat is part of the user's \"{}\" project.", p.name, p.name);
    if !p.instructions.trim().is_empty() {
        s.push_str(&format!("\nThe user's instructions for this project:\n{}", p.instructions.trim()));
    }
    s
}

// ---------- commands ----------

#[tauri::command]
pub fn list_projects(state: AppStateRef) -> Result<Vec<Project>, String> {
    let c = state.cipher()?;
    Ok(list(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
pub fn create_project(state: AppStateRef, name: String) -> Result<Project, String> {
    let c = state.cipher()?;
    let name = name.trim();
    if name.is_empty() {
        return Err("A project needs a name.".into());
    }
    let now = now_ms();
    let p = Project { id: uuid::Uuid::new_v4().to_string(), name: name.into(), instructions: String::new(), folders: Vec::new(), created_at: now, updated_at: now };
    let conn = state.db.lock().unwrap();
    conn.execute(
        "INSERT INTO projects (id, name, instructions, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![p.id, c.encrypt(&p.name), c.encrypt(""), now],
    )
    .map_err(err)?;
    db::log_action(&conn, "chat", "Created a project");
    Ok(p)
}

#[tauri::command]
pub fn update_project(state: AppStateRef, id: String, name: String, instructions: String) -> Result<(), String> {
    let c = state.cipher()?;
    let name = name.trim();
    if name.is_empty() {
        return Err("A project needs a name.".into());
    }
    if instructions.chars().count() > 8000 {
        return Err("Keep project instructions under 8,000 characters.".into());
    }
    let conn = state.db.lock().unwrap();
    let n = conn
        .execute(
            "UPDATE projects SET name = ?2, instructions = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, c.encrypt(name), c.encrypt(&instructions), now_ms()],
        )
        .map_err(err)?;
    if n == 0 {
        return Err("That project no longer exists.".into());
    }
    Ok(())
}

/// Deletes the project and its memories. Its chats stay, outside any project.
#[tauri::command]
pub fn delete_project(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("UPDATE chats SET project_id = NULL WHERE project_id = ?1", [&id]).map_err(err)?;
    conn.execute("DELETE FROM memories WHERE project_id = ?1", [&id]).map_err(err)?;
    conn.execute("DELETE FROM projects WHERE id = ?1", [&id]).map_err(err)?;
    db::log_action(&conn, "chat", "Deleted a project and its memories (its chats were kept)");
    Ok(())
}

#[tauri::command]
pub fn add_project_folder(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    state.cipher()?;
    if let Some(reason) = sandbox::refuse_reason(Path::new(&path), &state.paths.data) {
        return Err(reason.into());
    }
    let canonical = dunce::canonicalize(&path).map_err(|e| e.to_string())?.display().to_string();
    let conn = state.db.lock().unwrap();
    if !exists(&conn, &id) {
        return Err("That project no longer exists.".into());
    }
    conn.execute("INSERT OR IGNORE INTO project_folders (project_id, path) VALUES (?1, ?2)", params![id, canonical]).map_err(err)?;
    db::log_action(&conn, "privacy", "Shared a folder with a project");
    Ok(())
}

#[tauri::command]
pub fn remove_project_folder(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM project_folders WHERE project_id = ?1 AND path = ?2", params![id, path]).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn create_chat_in(state: AppStateRef, project_id: Option<String>, incognito: bool) -> Result<Chat, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    if let Some(pid) = &project_id {
        if !exists(&conn, pid) {
            return Err("That project no longer exists.".into());
        }
    }
    // Only one incognito chat at a time; leaving one deletes it.
    db::delete_incognito_chats(&conn, None)?;
    // A new chat that was never written in is replaced, not kept.
    db::delete_empty_chats(&conn)?;
    let model = db::settings(&conn).default_model;
    db::create_chat_in(&conn, &c, model, project_id, incognito)
}

#[tauri::command]
pub fn leave_incognito(state: AppStateRef, keep: Option<String>) -> Result<(), String> {
    state.cipher()?;
    db::delete_incognito_chats(&state.db.lock().unwrap(), keep.as_deref())?;
    Ok(())
}

#[tauri::command]
pub fn set_chat_project(state: AppStateRef, chat_id: String, project_id: Option<String>) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    if let Some(pid) = &project_id {
        if !exists(&conn, pid) {
            return Err("That project no longer exists.".into());
        }
    }
    conn.execute("UPDATE chats SET project_id = ?2 WHERE id = ?1", params![chat_id, project_id]).map_err(err)?;
    Ok(())
}

// ---------- memory commands ----------

#[tauri::command]
pub fn list_memories(state: AppStateRef) -> Result<Vec<Memory>, String> {
    let c = state.cipher()?;
    Ok(memory::list(&state.db.lock().unwrap(), &c, None))
}

#[tauri::command]
pub fn add_memory(state: AppStateRef, content: String, project_id: Option<String>) -> Result<Memory, String> {
    let c = state.cipher()?;
    Ok(memory::add(&state.db.lock().unwrap(), &c, &content, project_id.as_deref(), None)?.0)
}

#[tauri::command]
pub fn update_memory(state: AppStateRef, id: String, content: String) -> Result<(), String> {
    let c = state.cipher()?;
    memory::update(&state.db.lock().unwrap(), &c, &id, &content)
}

#[tauri::command]
pub fn delete_memory(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    memory::delete(&conn, &id)?;
    Ok(())
}

#[tauri::command]
pub fn clear_memories(state: AppStateRef) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    let n = memory::clear(&conn)?;
    db::log_action(&conn, "privacy", &format!("Deleted all {n} memories"));
    Ok(())
}

#[tauri::command]
pub fn set_memory_enabled(state: AppStateRef, enabled: bool) -> Result<db::Settings, String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    let s = db::update_settings(&conn, |s| s.memory_enabled = enabled)?;
    db::log_action(&conn, "privacy", if enabled { "Turned memory on" } else { "Turned memory off" });
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_folders_add_the_project_folders() {
        let d = std::env::temp_dir().join(format!("sulcusai-proj-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let conn = db::open(&d.join("t.db")).unwrap();
        db::add_folder(&conn, "C:\\shared").unwrap();
        conn.execute("INSERT INTO projects VALUES ('p', 'n', 'i', 1, 1)", []).unwrap();
        conn.execute("INSERT INTO project_folders VALUES ('p', 'C:\\proj')", []).unwrap();
        let mut chat = Chat {
            id: "c".into(),
            title: String::new(),
            model_id: None,
            web: false,
            created_at: 0,
            updated_at: 0,
            mode: "auto".into(),
            project_id: None,
            incognito: false,
            parent_id: None,
            empty: false,
        };
        assert_eq!(chat_folders(&conn, &chat), vec!["C:\\shared"]);
        chat.project_id = Some("p".into());
        assert_eq!(chat_folders(&conn, &chat), vec!["C:\\shared", "C:\\proj"]);
    }

    #[test]
    fn project_prompt_includes_instructions() {
        let p = Project { id: "p".into(), name: "Thesis".into(), instructions: "Use APA style.".into(), folders: vec![], created_at: 0, updated_at: 0 };
        let s = prompt_section(&p);
        assert!(s.contains("Thesis") && s.contains("APA"));
    }
}

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

/// Folders a chat may use: its own (shared in it), its project's, then the
/// ones shared with every chat.
pub fn chat_folders(conn: &Connection, chat: &Chat) -> Vec<String> {
    let mut all = own_folders(conn, chat);
    for f in db::folders(conn) {
        if !all.iter().any(|a| a.eq_ignore_ascii_case(&f)) {
            all.push(f);
        }
    }
    all
}

/// The folders that are this chat's work: shared in it, or its project's.
pub fn own_folders(conn: &Connection, chat: &Chat) -> Vec<String> {
    let mut own = db::chat_shared_folders(conn, &chat.id);
    if let Some(pid) = &chat.project_id {
        for f in folders(conn, pid) {
            if !own.iter().any(|a| a.eq_ignore_ascii_case(&f)) {
                own.push(f);
            }
        }
    }
    own
}

/// The folders shared with every chat that aren't also this chat's own:
/// the model is told they're other work, not to be looked through unasked.
pub fn general_folders(conn: &Connection, chat: &Chat) -> Vec<String> {
    let own = own_folders(conn, chat);
    db::folders(conn).into_iter().filter(|f| !own.iter().any(|o| o.eq_ignore_ascii_case(f))).collect()
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

#[tauri::command(async)]
pub fn list_projects(state: AppStateRef) -> Result<Vec<Project>, String> {
    let c = state.cipher()?;
    Ok(list(&state.db.lock().unwrap(), &c))
}

#[tauri::command(async)]
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

#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn delete_project(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("UPDATE chats SET project_id = NULL WHERE project_id = ?1", [&id]).map_err(err)?;
    conn.execute("DELETE FROM memories WHERE project_id = ?1", [&id]).map_err(err)?;
    conn.execute("DELETE FROM projects WHERE id = ?1", [&id]).map_err(err)?;
    db::log_action(&conn, "chat", "Deleted a project and its memories (its chats were kept)");
    Ok(())
}

#[tauri::command(async)]
pub fn add_project_folder(state: AppStateRef, id: String, path: String, allow_system: Option<bool>) -> Result<(), String> {
    state.cipher()?;
    if let Some(reason) = sandbox::refuse_reason(Path::new(&path), &state.paths.data, allow_system.unwrap_or(false)) {
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

#[tauri::command(async)]
pub fn remove_project_folder(state: AppStateRef, id: String, path: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM project_folders WHERE project_id = ?1 AND path = ?2", params![id, path]).map_err(err)?;
    Ok(())
}

#[tauri::command(async)]
pub fn create_chat_in(state: AppStateRef, project_id: Option<String>, incognito: bool) -> Result<Chat, String> {
    let c = state.cipher()?;
    // Only one incognito chat at a time; leaving one deletes it, with what it left.
    crate::clear_incognito(&state, None)?;
    let conn = state.db.lock().unwrap();
    if let Some(pid) = &project_id {
        if !exists(&conn, pid) {
            return Err("That project no longer exists.".into());
        }
    }
    // A new chat that was never written in is replaced, not kept.
    db::delete_empty_chats(&conn)?;
    let model = db::settings(&conn).default_model;
    db::create_chat_in(&conn, &c, model, project_id, incognito)
}

#[tauri::command(async)]
pub fn leave_incognito(state: AppStateRef, keep: Option<String>) -> Result<(), String> {
    state.cipher()?;
    crate::clear_incognito(&state, keep.as_deref())
}

#[tauri::command(async)]
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

#[tauri::command(async)]
pub fn list_memories(state: AppStateRef) -> Result<Vec<Memory>, String> {
    let c = state.cipher()?;
    Ok(memory::list(&state.db.lock().unwrap(), &c, None))
}

/// A memory the user adds: everywhere, in a project, and (added from a
/// chat) kept with that chat's memories.
#[tauri::command(async)]
pub fn add_memory(state: AppStateRef, content: String, project_id: Option<String>, chat_id: Option<String>) -> Result<Memory, String> {
    let c = state.cipher()?;
    Ok(memory::add(&state.db.lock().unwrap(), &c, &content, project_id.as_deref(), chat_id.as_deref())?.0)
}

#[tauri::command(async)]
pub fn update_memory(state: AppStateRef, id: String, content: String) -> Result<(), String> {
    let c = state.cipher()?;
    memory::update(&state.db.lock().unwrap(), &c, &id, &content)
}

/// Where a memory applies: everywhere (None) or one project.
#[tauri::command(async)]
pub fn set_memory_scope(state: AppStateRef, id: String, project_id: Option<String>) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    let n = conn.execute("UPDATE memories SET project_id = ?2, updated_at = ?3 WHERE id = ?1", rusqlite::params![id, project_id, crate::db::now_ms()]).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("That memory no longer exists.".into());
    }
    Ok(())
}

/// Whether the assistant saves lasting facts from chats by itself (after a
/// reply, with the model on this PC).
#[tauri::command(async)]
pub fn memory_auto(state: AppStateRef) -> bool {
    memory::auto_on(&state.db.lock().unwrap())
}

#[tauri::command(async)]
pub fn set_memory_auto(state: AppStateRef, on: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    crate::db::set(&conn, memory::AUTO_KEY, &on)?;
    crate::db::log_action(&conn, "settings", if on { "Memories from chats: saved automatically" } else { "Memories from chats: only when asked" });
    Ok(())
}

#[tauri::command(async)]
pub fn delete_memory(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    memory::delete(&conn, &id)?;
    Ok(())
}

#[tauri::command(async)]
pub fn clear_memories(state: AppStateRef) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    let n = memory::clear(&conn)?;
    db::log_action(&conn, "privacy", &format!("Deleted all {n} memories"));
    Ok(())
}

#[tauri::command(async)]
pub fn set_memory_enabled(state: AppStateRef, enabled: bool) -> Result<db::Settings, String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    let s = db::update_settings(&conn, |s| s.memory_enabled = enabled)?;
    db::log_action(&conn, "privacy", if enabled { "Turned memory on" } else { "Turned memory off" });
    Ok(s)
}

// ---------- linked chats ----------

/// Pairs of chats the user linked to work together: each knows the other
/// and can read it and send it a message, with or without the Chats working
/// together feature.
const LINKS: &str = "chat_links";

fn links(conn: &Connection) -> Vec<(String, String)> {
    db::get(conn, LINKS).unwrap_or_default()
}

/// The chats linked with `chat_id` that still exist (incognito ones never).
pub fn linked(conn: &Connection, c: &Cipher, chat_id: &str) -> Vec<Chat> {
    links(conn)
        .into_iter()
        .filter_map(|(a, b)| if a == chat_id { Some(b) } else if b == chat_id { Some(a) } else { None })
        .filter_map(|id| db::chat(conn, c, &id))
        .filter(|ch| !ch.incognito)
        .collect()
}

#[tauri::command(async)]
pub fn linked_chats(state: AppStateRef, chat_id: String) -> Result<Vec<Chat>, String> {
    let c = state.cipher()?;
    Ok(linked(&state.db.lock().unwrap(), &c, &chat_id))
}

#[tauri::command(async)]
pub fn link_chats(state: AppStateRef, a: String, b: String) -> Result<(), String> {
    let c = state.cipher()?;
    if a == b {
        return Err("A chat can't be linked with itself.".into());
    }
    let conn = state.db.lock().unwrap();
    for id in [&a, &b] {
        match db::chat(&conn, &c, id) {
            Some(ch) if ch.incognito => return Err("Incognito chats can't be linked: they keep nothing.".into()),
            Some(_) => {}
            None => return Err("That chat no longer exists.".into()),
        }
    }
    let mut all = links(&conn);
    if !all.iter().any(|(x, y)| (x == &a && y == &b) || (x == &b && y == &a)) {
        all.push((a, b));
        db::set(&conn, LINKS, &all)?;
        db::log_action(&conn, "chat", "Linked two chats to work together");
    }
    Ok(())
}

#[tauri::command(async)]
pub fn unlink_chats(state: AppStateRef, a: String, b: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = links(&conn);
    all.retain(|(x, y)| !((x == &a && y == &b) || (x == &b && y == &a)));
    db::set(&conn, LINKS, &all)
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
        assert_eq!(chat_folders(&conn, &chat), vec!["C:\\proj", "C:\\shared"], "the chat's own work first");
        assert_eq!(general_folders(&conn, &chat), vec!["C:\\shared"]);
        // Shared in this chat only: first, and no other chat has it.
        conn.execute("INSERT INTO chats (id, title, created_at, updated_at) VALUES ('c', '', 0, 0), ('d', '', 0, 0)", []).unwrap();
        db::add_chat_folder(&conn, "c", "C:\\gun").unwrap();
        assert_eq!(chat_folders(&conn, &chat), vec!["C:\\gun", "C:\\proj", "C:\\shared"]);
        assert_eq!(own_folders(&conn, &chat), vec!["C:\\gun", "C:\\proj"]);
        let other = Chat { id: "d".into(), project_id: None, ..chat.clone() };
        assert_eq!(chat_folders(&conn, &other), vec!["C:\\shared"]);
        // Gone with its chat.
        db::delete_chat(&conn, "c").unwrap();
        assert!(db::chat_shared_folders(&conn, "c").is_empty());
    }

    #[test]
    fn project_prompt_includes_instructions() {
        let p = Project { id: "p".into(), name: "Thesis".into(), instructions: "Use APA style.".into(), folders: vec![], created_at: 0, updated_at: 0 };
        let s = prompt_section(&p);
        assert!(s.contains("Thesis") && s.contains("APA"));
    }
}

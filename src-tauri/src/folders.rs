// SPDX-License-Identifier: AGPL-3.0-only
//! Chat folders: the person sorts chats into named folders in the sidebar,
//! can lock a folder (the automatic cleanup keeps every chat in it) and can
//! share a whole folder with people they're linked with: every chat in it
//! is shared with them, and chats moved in later are too.
//!
//! Kept in this PC's settings (names encrypted), not synced to other PCs.

use std::collections::HashSet;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::crypto::Cipher;
use crate::features::{self, Feature};
use crate::{db, AppState, AppStateRef};

const KEY: &str = "chat_folders";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Folder {
    id: String,
    /// The name, encrypted with the data key.
    name: String,
    chats: Vec<String>,
    locked: bool,
    /// People every chat in it is shared with (contact ids).
    shared_with: Vec<String>,
    collapsed: bool,
    created_at: i64,
}

fn load(conn: &Connection) -> Vec<Folder> {
    db::get(conn, KEY).unwrap_or_default()
}

fn save(conn: &Connection, all: &[Folder]) -> Result<(), String> {
    db::set(conn, KEY, &all)
}

/// Chats the automatic cleanup must keep: those in locked or shared folders.
pub fn kept_chats(conn: &Connection) -> HashSet<String> {
    load(conn).into_iter().filter(|f| f.locked || !f.shared_with.is_empty()).flat_map(|f| f.chats).collect()
}

/// A chat was deleted: it leaves its folder.
pub fn forget_chat(conn: &Connection, chat_id: &str) {
    let mut all = load(conn);
    let before: usize = all.iter().map(|f| f.chats.len()).sum();
    for f in all.iter_mut() {
        f.chats.retain(|c| c != chat_id);
    }
    if all.iter().map(|f| f.chats.len()).sum::<usize>() != before {
        let _ = save(conn, &all);
    }
}

#[derive(Serialize)]
pub struct Person {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct FolderView {
    pub id: String,
    pub name: String,
    pub chats: Vec<String>,
    pub locked: bool,
    pub shared_with: Vec<Person>,
    pub collapsed: bool,
}

fn view(conn: &Connection, c: &Cipher) -> Vec<FolderView> {
    let people = crate::sync::people::contact_names(conn);
    load(conn)
        .into_iter()
        .map(|f| FolderView {
            name: c.decrypt_or(&f.name, "Folder"),
            shared_with: f.shared_with.iter().map(|id| Person { name: people.iter().find(|(p, _)| p == id).map_or_else(|| "Someone".into(), |(_, n)| n.clone()), id: id.clone() }).collect(),
            id: f.id,
            chats: f.chats,
            locked: f.locked,
            collapsed: f.collapsed,
        })
        .collect()
}

fn change(state: &AppState, f: impl FnOnce(&mut Vec<Folder>, &Cipher) -> Result<(), String>) -> Result<Vec<FolderView>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn);
    f(&mut all, &c)?;
    save(&conn, &all)?;
    Ok(view(&conn, &c))
}

fn clean_name(name: &str) -> Result<String, String> {
    let n: String = name.trim().chars().take(60).collect();
    if n.is_empty() {
        return Err("Give the folder a name.".into());
    }
    Ok(n)
}

#[tauri::command(async)]
pub fn folders_view(state: AppStateRef) -> Result<Vec<FolderView>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    Ok(view(&conn, &c))
}

/// A new folder; with `chat_id`, that chat moves into it.
#[tauri::command(async)]
pub fn create_folder(state: AppStateRef, name: String, chat_id: Option<String>) -> Result<Vec<FolderView>, String> {
    let name = clean_name(&name)?;
    let id = uuid::Uuid::new_v4().to_string();
    change(&state, |all, c| {
        for f in all.iter_mut() {
            if let Some(chat) = &chat_id {
                f.chats.retain(|x| x != chat);
            }
        }
        all.push(Folder { id: id.clone(), name: c.encrypt(&name), chats: chat_id.into_iter().collect(), created_at: db::now_ms(), ..Default::default() });
        Ok(())
    })
}

/// Puts chats into the folder called `name`, made if there isn't one (an
/// import's chats go together).
pub(crate) fn put_chats_in(state: &AppState, name: &str, chats: &[String]) -> Result<(), String> {
    let name = clean_name(name)?;
    change(state, |all, c| {
        let i = match all.iter().position(|f| c.decrypt(&f.name).ok().as_deref() == Some(name.as_str())) {
            Some(i) => i,
            None => {
                all.push(Folder { id: uuid::Uuid::new_v4().to_string(), name: c.encrypt(&name), created_at: db::now_ms(), ..Default::default() });
                all.len() - 1
            }
        };
        for chat in chats {
            if !all[i].chats.contains(chat) {
                all[i].chats.push(chat.clone());
            }
        }
        Ok(())
    })
    .map(|_| ())
}

#[tauri::command(async)]
pub fn rename_folder(state: AppStateRef, id: String, name: String) -> Result<Vec<FolderView>, String> {
    let name = clean_name(&name)?;
    change(&state, |all, c| {
        let f = all.iter_mut().find(|f| f.id == id).ok_or("That folder no longer exists.")?;
        f.name = c.encrypt(&name);
        Ok(())
    })
}

#[tauri::command(async)]
pub fn set_folder_flags(state: AppStateRef, id: String, locked: Option<bool>, collapsed: Option<bool>) -> Result<Vec<FolderView>, String> {
    change(&state, |all, _| {
        let f = all.iter_mut().find(|f| f.id == id).ok_or("That folder no longer exists.")?;
        if let Some(l) = locked {
            f.locked = l;
        }
        if let Some(c) = collapsed {
            f.collapsed = c;
        }
        Ok(())
    })
}

/// Shares (or stops sharing) one chat with people, keeping anyone it's
/// shared with on its own.
fn reshare(state: &AppState, c: &Cipher, chat_id: &str, add: &[String], drop: &[String]) {
    let now = crate::sync::people::share::sharing_of(&state.db.lock().unwrap(), chat_id);
    let mut with: Vec<String> = now.into_iter().filter(|p| !drop.contains(p)).collect();
    for p in add {
        if !with.contains(p) {
            with.push(p.clone());
        }
    }
    if let Err(e) = crate::sync::people::share::set_chat_sharing(state, c, chat_id, with) {
        crate::applog::line("people", &format!("Folder sharing: a chat wasn't shared: {e}"));
    }
}

/// Deletes a folder. Its chats stay (back in the main list); if the folder
/// was shared, its chats stop being shared with those people.
#[tauri::command(async)]
pub fn delete_folder(state: AppStateRef, id: String) -> Result<Vec<FolderView>, String> {
    let c = state.cipher()?;
    let gone = {
        let conn = state.db.lock().unwrap();
        let mut all = load(&conn);
        let gone = all.iter().find(|f| f.id == id).cloned();
        all.retain(|f| f.id != id);
        save(&conn, &all)?;
        gone
    };
    if let Some(f) = gone.filter(|f| !f.shared_with.is_empty()) {
        for chat in &f.chats {
            reshare(&state, &c, chat, &[], &f.shared_with);
        }
    }
    folders_view(state)
}

/// Moves a chat into a folder (None: out of any folder). Moving into a
/// shared folder shares it with those people; moving out stops that.
#[tauri::command(async)]
pub fn move_to_folder(state: AppStateRef, chat_id: String, folder_id: Option<String>) -> Result<Vec<FolderView>, String> {
    let c = state.cipher()?;
    let (left, joined) = {
        let conn = state.db.lock().unwrap();
        let mut all = load(&conn);
        if let Some(id) = &folder_id {
            if !all.iter().any(|f| &f.id == id) {
                return Err("That folder no longer exists.".into());
            }
        }
        let mut left: Vec<String> = Vec::new();
        let mut joined: Vec<String> = Vec::new();
        for f in all.iter_mut() {
            let was = f.chats.contains(&chat_id);
            let is = folder_id.as_deref() == Some(f.id.as_str());
            if was && !is {
                f.chats.retain(|x| x != &chat_id);
                left.extend(f.shared_with.iter().cloned());
            }
            if is && !was {
                f.chats.push(chat_id.clone());
                joined.extend(f.shared_with.iter().cloned());
            }
        }
        save(&conn, &all)?;
        (left, joined)
    };
    if !left.is_empty() || !joined.is_empty() {
        if !joined.is_empty() {
            features::require(&state, Feature::People)?;
        }
        let drop: Vec<String> = left.into_iter().filter(|p| !joined.contains(p)).collect();
        reshare(&state, &c, &chat_id, &joined, &drop);
    }
    folders_view(state)
}

#[derive(Serialize)]
pub struct FolderShareChoice {
    pub id: String,
    pub name: String,
    pub shared: bool,
}

/// The people a folder can be shared with, and whether it is.
#[tauri::command(async)]
pub fn folder_sharing(state: AppStateRef, id: String) -> Result<Vec<FolderShareChoice>, String> {
    features::require(&state, Feature::People)?;
    let conn = state.db.lock().unwrap();
    let f = load(&conn).into_iter().find(|f| f.id == id).ok_or("That folder no longer exists.")?;
    Ok(crate::sync::people::contact_names(&conn).into_iter().map(|(pid, name)| FolderShareChoice { shared: f.shared_with.contains(&pid), id: pid, name }).collect())
}

/// Shares a folder with exactly these people: every chat in it is shared
/// with them (incognito chats can't be), and chats moved in later too.
#[tauri::command(async)]
pub fn share_folder(state: AppStateRef, id: String, with: Vec<String>) -> Result<Vec<FolderShareChoice>, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    let (f, known) = {
        let conn = state.db.lock().unwrap();
        let known: Vec<String> = crate::sync::people::contact_names(&conn).into_iter().map(|(p, _)| p).collect();
        let mut all = load(&conn);
        let f = all.iter_mut().find(|f| f.id == id).ok_or("That folder no longer exists.")?;
        let before = f.clone();
        f.shared_with = with.iter().filter(|w| known.contains(w)).cloned().collect();
        let after = f.clone();
        save(&conn, &all)?;
        ((before, after), known)
    };
    let (before, after) = f;
    let add: Vec<String> = after.shared_with.iter().filter(|p| !before.shared_with.contains(p)).cloned().collect();
    let drop: Vec<String> = before.shared_with.iter().filter(|p| !after.shared_with.contains(p) && known.contains(p)).cloned().collect();
    for chat in &after.chats {
        reshare(&state, &c, chat, &add, &drop);
    }
    {
        let conn = state.db.lock().unwrap();
        let summary = if after.shared_with.is_empty() { "Stopped sharing a chat folder".to_string() } else { format!("Shared a chat folder ({} chats) with {} people", after.chats.len(), after.shared_with.len()) };
        db::log_action(&conn, "people", &summary);
    }
    folder_sharing(state, id)
}

/// A folder holding one chat (tests).
#[cfg(test)]
pub fn create_for_test(state: &AppState, name: &str, chat_id: &str, locked: bool) {
    let c = state.cipher().unwrap();
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn);
    all.push(Folder { id: uuid::Uuid::new_v4().to_string(), name: c.encrypt(name), chats: vec![chat_id.to_string()], locked, created_at: db::now_ms(), ..Default::default() });
    save(&conn, &all).unwrap();
}

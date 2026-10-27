// SPDX-License-Identifier: AGPL-3.0-only
//! Saved prompts: text the person uses often, typed in a chat as /name.
//! Kept encrypted like chats, since they can hold anything.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::crypto::Cipher;
use crate::{db, AppStateRef};

const KEY: &str = "saved_prompts";
/// Names the chat box already uses.
const TAKEN: &[&str] = &["compact"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedPrompt {
    pub id: String,
    /// Typed after the slash: small letters, digits and dashes.
    pub name: String,
    pub text: String,
}

fn load(conn: &Connection, c: &Cipher) -> Vec<SavedPrompt> {
    db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
}

fn store(conn: &Connection, c: &Cipher, all: &[SavedPrompt]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

/// A name as it's typed after the slash ("Fix Grammar!" → "fix-grammar").
pub fn slash_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().trim_start_matches('/').chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

#[tauri::command(async)]
pub fn saved_prompts(state: AppStateRef) -> Result<Vec<SavedPrompt>, String> {
    let c = state.cipher()?;
    Ok(load(&state.db.lock().unwrap(), &c))
}

/// Adds a prompt (`id` empty) or changes one; gives them all back.
#[tauri::command(async)]
pub fn save_prompt(state: AppStateRef, id: String, name: String, text: String) -> Result<Vec<SavedPrompt>, String> {
    let c = state.cipher()?;
    let name = slash_name(&name);
    if name.is_empty() || text.trim().is_empty() {
        return Err("A saved prompt needs a name and its text.".into());
    }
    if TAKEN.contains(&name.as_str()) {
        return Err(format!("/{name} is already a command. Pick another name."));
    }
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn, &c);
    if all.iter().any(|p| p.name == name && p.id != id) {
        return Err(format!("Another saved prompt is called /{name}."));
    }
    match all.iter_mut().find(|p| p.id == id && !id.is_empty()) {
        Some(p) => {
            p.name = name;
            p.text = text;
        }
        None => all.push(SavedPrompt { id: uuid::Uuid::new_v4().to_string(), name, text }),
    }
    all.sort_by(|a, b| a.name.cmp(&b.name));
    store(&conn, &c, &all)?;
    Ok(all)
}

#[tauri::command(async)]
pub fn delete_prompt(state: AppStateRef, id: String) -> Result<Vec<SavedPrompt>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let mut all = load(&conn, &c);
    all.retain(|p| p.id != id);
    store(&conn, &c, &all)?;
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_what_follows_the_slash() {
        assert_eq!(slash_name("Fix Grammar!"), "fix-grammar");
        assert_eq!(slash_name("/summary"), "summary");
        assert_eq!(slash_name("  Résumé  tips "), "résumé-tips");
        assert_eq!(slash_name("!!"), "");
    }

    #[test]
    fn prompts_are_kept_encrypted() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = Cipher::random();
        let p = SavedPrompt { id: "1".into(), name: "tldr".into(), text: "Sum this up in three lines.".into() };
        store(&conn, &c, std::slice::from_ref(&p)).unwrap();
        assert_eq!(load(&conn, &c), vec![p]);
        let raw: String = db::get(&conn, KEY).unwrap();
        assert!(!raw.contains("three lines"));
    }
}

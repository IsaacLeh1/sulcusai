// SPDX-License-Identifier: AGPL-3.0-only
//! Local SQLite storage: settings, installed models, chats and messages.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::net::Connectivity;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS installed_models (
  model_id     TEXT PRIMARY KEY,
  quant        TEXT NOT NULL,
  path         TEXT NOT NULL,
  size         INTEGER NOT NULL,
  installed_at INTEGER NOT NULL,
  tps          REAL
);
CREATE TABLE IF NOT EXISTS chats (
  id         TEXT PRIMARY KEY,
  title      TEXT NOT NULL,
  model_id   TEXT,
  web        INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
  id         TEXT PRIMARY KEY,
  chat_id    TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  role       TEXT NOT NULL,
  content    TEXT NOT NULL,
  thinking   TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_by_chat ON messages(chat_id, created_at);
";

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn get<T: DeserializeOwned>(conn: &Connection, key: &str) -> Option<T> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

pub fn set<T: Serialize>(conn: &Connection, key: &str, value: &T) -> Result<(), String> {
    let json = serde_json::to_string(value).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub connectivity: Connectivity,
    pub default_model: Option<String>,
}

pub fn settings(conn: &Connection) -> Settings {
    get(conn, "settings").unwrap_or_default()
}

/// What the user types about themselves; included in every chat.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    pub name: String,
    pub about: String,
    pub preferences: String,
}

pub fn profile(conn: &Connection) -> Profile {
    get(conn, "profile").unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledModel {
    pub model_id: String,
    pub quant: String,
    pub path: String,
    pub size: u64,
    pub installed_at: i64,
    pub tps: Option<f64>,
}

pub fn installed_models(conn: &Connection) -> Vec<InstalledModel> {
    let mut stmt = match conn.prepare(
        "SELECT model_id, quant, path, size, installed_at, tps FROM installed_models ORDER BY installed_at",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |r| {
        Ok(InstalledModel {
            model_id: r.get(0)?,
            quant: r.get(1)?,
            path: r.get(2)?,
            size: r.get::<_, i64>(3)? as u64,
            installed_at: r.get(4)?,
            tps: r.get(5)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn installed_model(conn: &Connection, model_id: &str) -> Option<InstalledModel> {
    installed_models(conn).into_iter().find(|m| m.model_id == model_id)
}

pub fn save_installed(conn: &Connection, m: &InstalledModel) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO installed_models (model_id, quant, path, size, installed_at, tps)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![m.model_id, m.quant, m.path, m.size as i64, m.installed_at, m.tps],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_tps(conn: &Connection, model_id: &str, tps: f64) -> Result<(), String> {
    conn.execute("UPDATE installed_models SET tps = ?2 WHERE model_id = ?1", params![model_id, tps])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn remove_installed(conn: &Connection, model_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM installed_models WHERE model_id = ?1", [model_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub model_id: Option<String>,
    pub web: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn chat_from_row(r: &rusqlite::Row) -> rusqlite::Result<Chat> {
    Ok(Chat {
        id: r.get(0)?,
        title: r.get(1)?,
        model_id: r.get(2)?,
        web: r.get::<_, i64>(3)? != 0,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

const CHAT_COLS: &str = "id, title, model_id, web, created_at, updated_at";

pub fn list_chats(conn: &Connection) -> Vec<Chat> {
    let sql = format!("SELECT {CHAT_COLS} FROM chats ORDER BY updated_at DESC");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map([], chat_from_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn chat(conn: &Connection, id: &str) -> Option<Chat> {
    let sql = format!("SELECT {CHAT_COLS} FROM chats WHERE id = ?1");
    conn.query_row(&sql, [id], chat_from_row).optional().ok().flatten()
}

pub fn create_chat(conn: &Connection, model_id: Option<String>) -> Result<Chat, String> {
    let now = now_ms();
    let chat = Chat {
        id: uuid::Uuid::new_v4().to_string(),
        title: "New chat".into(),
        model_id,
        web: false,
        created_at: now,
        updated_at: now,
    };
    conn.execute(
        "INSERT INTO chats (id, title, model_id, web, created_at, updated_at) VALUES (?1, ?2, ?3, 0, ?4, ?4)",
        params![chat.id, chat.title, chat.model_id, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(chat)
}

pub fn update_chat(conn: &Connection, id: &str, title: Option<&str>, model_id: Option<&str>, web: Option<bool>) -> Result<(), String> {
    let now = now_ms();
    if let Some(t) = title {
        conn.execute("UPDATE chats SET title = ?2 WHERE id = ?1", params![id, t]).map_err(|e| e.to_string())?;
    }
    if let Some(m) = model_id {
        conn.execute("UPDATE chats SET model_id = ?2 WHERE id = ?1", params![id, m]).map_err(|e| e.to_string())?;
    }
    if let Some(w) = web {
        conn.execute("UPDATE chats SET web = ?2 WHERE id = ?1", params![id, w as i64]).map_err(|e| e.to_string())?;
    }
    conn.execute("UPDATE chats SET updated_at = ?2 WHERE id = ?1", params![id, now]).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_chat(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM chats WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub role: String,
    pub content: String,
    pub thinking: Option<String>,
    pub created_at: i64,
}

pub fn messages(conn: &Connection, chat_id: &str) -> Vec<Message> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, chat_id, role, content, thinking, created_at FROM messages WHERE chat_id = ?1 ORDER BY created_at, rowid",
    ) else {
        return Vec::new();
    };
    stmt.query_map([chat_id], |r| {
        Ok(Message {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            role: r.get(2)?,
            content: r.get(3)?,
            thinking: r.get(4)?,
            created_at: r.get(5)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn add_message(conn: &Connection, m: &Message) -> Result<(), String> {
    conn.execute(
        "INSERT INTO messages (id, chat_id, role, content, thinking, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![m.id, m.chat_id, m.role, m.content, m.thinking, m.created_at],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("UPDATE chats SET updated_at = ?2 WHERE id = ?1", params![m.chat_id, m.created_at])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    #[test]
    fn settings_default_to_offline() {
        let conn = mem();
        assert_eq!(settings(&conn).connectivity, Connectivity::Offline);
        set(&conn, "settings", &Settings { connectivity: Connectivity::Web, default_model: None }).unwrap();
        assert_eq!(settings(&conn).connectivity, Connectivity::Web);
    }

    #[test]
    fn deleting_a_chat_deletes_its_messages() {
        let conn = mem();
        let chat = create_chat(&conn, None).unwrap();
        add_message(&conn, &Message {
            id: "m1".into(),
            chat_id: chat.id.clone(),
            role: "user".into(),
            content: "hi".into(),
            thinking: None,
            created_at: now_ms(),
        })
        .unwrap();
        assert_eq!(messages(&conn, &chat.id).len(), 1);
        delete_chat(&conn, &chat.id).unwrap();
        assert!(messages(&conn, &chat.id).is_empty());
    }
}

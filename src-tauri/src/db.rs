// SPDX-License-Identifier: AGPL-3.0-only
//! Local SQLite storage: settings, installed models, chats, messages and the
//! action log. Chat titles, message text and the profile are stored encrypted
//! (see crypto.rs); ids, timestamps and settings are not.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::crypto::Cipher;
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
CREATE TABLE IF NOT EXISTS action_log (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  at       INTEGER NOT NULL,
  category TEXT NOT NULL,
  summary  TEXT NOT NULL
);
";

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // secure_delete overwrites deleted rows, so removed chats don't linger on disk.
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn get<T: DeserializeOwned>(conn: &Connection, key: &str) -> Option<T> {
    get_raw(conn, key).and_then(|s| serde_json::from_str(&s).ok())
}

fn get_raw(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

fn set_raw(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(err)?;
    Ok(())
}

pub fn set<T: Serialize>(conn: &Connection, key: &str, value: &T) -> Result<(), String> {
    set_raw(conn, key, &serde_json::to_string(value).map_err(err)?)
}

fn default_auto_lock() -> u32 {
    15
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub connectivity: Connectivity,
    #[serde(default)]
    pub default_model: Option<String>,
    /// The first-run setup has been finished or skipped.
    #[serde(default)]
    pub onboarded: bool,
    /// Minutes of inactivity before app lock engages (0 = never).
    #[serde(default = "default_auto_lock")]
    pub auto_lock_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { connectivity: Connectivity::Offline, default_model: None, onboarded: false, auto_lock_minutes: default_auto_lock() }
    }
}

pub fn settings(conn: &Connection) -> Settings {
    get(conn, "settings").unwrap_or_default()
}

pub fn update_settings(conn: &Connection, f: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
    let mut s = settings(conn);
    f(&mut s);
    set(conn, "settings", &s)?;
    Ok(s)
}

/// What the user types about themselves; included in every chat.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    pub name: String,
    pub about: String,
    pub preferences: String,
}

pub fn profile(conn: &Connection, c: &Cipher) -> Profile {
    get_raw(conn, "profile")
        .and_then(|raw| c.decrypt(&raw).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn set_profile(conn: &Connection, c: &Cipher, p: &Profile) -> Result<(), String> {
    set_raw(conn, "profile", &c.encrypt(&serde_json::to_string(p).map_err(err)?))
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
    let Ok(mut stmt) = conn.prepare(
        "SELECT model_id, quant, path, size, installed_at, tps FROM installed_models ORDER BY installed_at",
    ) else {
        return Vec::new();
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
    .map_err(err)?;
    Ok(())
}

pub fn set_tps(conn: &Connection, model_id: &str, tps: f64) -> Result<(), String> {
    conn.execute("UPDATE installed_models SET tps = ?2 WHERE model_id = ?1", params![model_id, tps]).map_err(err)?;
    Ok(())
}

pub fn remove_installed(conn: &Connection, model_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM installed_models WHERE model_id = ?1", [model_id]).map_err(err)?;
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

const CHAT_COLS: &str = "id, title, model_id, web, created_at, updated_at";

fn chat_from_row(c: &Cipher) -> impl Fn(&rusqlite::Row) -> rusqlite::Result<Chat> + '_ {
    move |r| {
        Ok(Chat {
            id: r.get(0)?,
            title: c.decrypt_or(&r.get::<_, String>(1)?, "(unreadable chat)"),
            model_id: r.get(2)?,
            web: r.get::<_, i64>(3)? != 0,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
        })
    }
}

pub fn list_chats(conn: &Connection, c: &Cipher) -> Vec<Chat> {
    let sql = format!("SELECT {CHAT_COLS} FROM chats ORDER BY updated_at DESC");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map([], chat_from_row(c))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn chat(conn: &Connection, c: &Cipher, id: &str) -> Option<Chat> {
    let sql = format!("SELECT {CHAT_COLS} FROM chats WHERE id = ?1");
    conn.query_row(&sql, [id], chat_from_row(c)).optional().ok().flatten()
}

pub fn create_chat(conn: &Connection, c: &Cipher, model_id: Option<String>) -> Result<Chat, String> {
    let now = now_ms();
    let chat = Chat { id: uuid::Uuid::new_v4().to_string(), title: "New chat".into(), model_id, web: false, created_at: now, updated_at: now };
    conn.execute(
        "INSERT INTO chats (id, title, model_id, web, created_at, updated_at) VALUES (?1, ?2, ?3, 0, ?4, ?4)",
        params![chat.id, c.encrypt(&chat.title), chat.model_id, now],
    )
    .map_err(err)?;
    Ok(chat)
}

pub fn set_chat_title(conn: &Connection, c: &Cipher, id: &str, title: &str) -> Result<(), String> {
    conn.execute("UPDATE chats SET title = ?2, updated_at = ?3 WHERE id = ?1", params![id, c.encrypt(title), now_ms()])
        .map_err(err)?;
    Ok(())
}

pub fn set_chat_model(conn: &Connection, id: &str, model_id: &str) -> Result<(), String> {
    conn.execute("UPDATE chats SET model_id = ?2 WHERE id = ?1", params![id, model_id]).map_err(err)?;
    Ok(())
}

pub fn set_chat_web(conn: &Connection, id: &str, web: bool) -> Result<(), String> {
    conn.execute("UPDATE chats SET web = ?2 WHERE id = ?1", params![id, web as i64]).map_err(err)?;
    Ok(())
}

pub fn delete_chat(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM chats WHERE id = ?1", [id]).map_err(err)?;
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

pub fn messages(conn: &Connection, c: &Cipher, chat_id: &str) -> Vec<Message> {
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
            content: c.decrypt_or(&r.get::<_, String>(3)?, "(This message couldn't be decrypted.)"),
            thinking: r.get::<_, Option<String>>(4)?.map(|t| c.decrypt_or(&t, "")),
            created_at: r.get(5)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn message_count(conn: &Connection, chat_id: &str) -> usize {
    conn.query_row("SELECT COUNT(*) FROM messages WHERE chat_id = ?1", [chat_id], |r| r.get::<_, i64>(0))
        .map(|n| n as usize)
        .unwrap_or(0)
}

pub fn add_message(conn: &Connection, c: &Cipher, m: &Message) -> Result<(), String> {
    conn.execute(
        "INSERT INTO messages (id, chat_id, role, content, thinking, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![m.id, m.chat_id, m.role, c.encrypt(&m.content), m.thinking.as_deref().map(|t| c.encrypt(t)), m.created_at],
    )
    .map_err(err)?;
    conn.execute("UPDATE chats SET updated_at = ?2 WHERE id = ?1", params![m.chat_id, m.created_at]).map_err(err)?;
    Ok(())
}

/// Encrypts anything saved before encryption existed. Returns how many
/// values changed; the caller vacuums afterwards so no plaintext remains.
pub fn encrypt_legacy(conn: &mut Connection, c: &Cipher) -> Result<usize, String> {
    let tx = conn.transaction().map_err(err)?;
    let mut changed = 0;
    {
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut stmt = tx.prepare("SELECT id, title FROM chats").map_err(err)?;
        for r in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)? {
            let (id, title): (String, String) = r.map_err(err)?;
            if !Cipher::is_encrypted(&title) {
                rows.push((id, title));
            }
        }
        for (id, title) in rows {
            tx.execute("UPDATE chats SET title = ?2 WHERE id = ?1", params![id, c.encrypt(&title)]).map_err(err)?;
            changed += 1;
        }

        let mut rows: Vec<(String, String, Option<String>)> = Vec::new();
        let mut stmt = tx.prepare("SELECT id, content, thinking FROM messages").map_err(err)?;
        for r in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(err)? {
            let (id, content, thinking): (String, String, Option<String>) = r.map_err(err)?;
            if !Cipher::is_encrypted(&content) || thinking.as_deref().is_some_and(|t| !Cipher::is_encrypted(t)) {
                rows.push((id, content, thinking));
            }
        }
        for (id, content, thinking) in rows {
            let content = if Cipher::is_encrypted(&content) { content } else { c.encrypt(&content) };
            let thinking = thinking.map(|t| if Cipher::is_encrypted(&t) { t } else { c.encrypt(&t) });
            tx.execute("UPDATE messages SET content = ?2, thinking = ?3 WHERE id = ?1", params![id, content, thinking])
                .map_err(err)?;
            changed += 1;
        }

        if let Some(raw) = get_raw(&tx, "profile").filter(|r| !Cipher::is_encrypted(r)) {
            set_raw(&tx, "profile", &c.encrypt(&raw))?;
            changed += 1;
        }
    }
    tx.commit().map_err(err)?;
    Ok(changed)
}

/// Rewrites the database file so freed pages (old plaintext) are gone.
pub fn compact(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;").map_err(err)
}

// ---------- action log ----------

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub id: i64,
    pub at: i64,
    pub category: String,
    pub summary: String,
}

/// Records something the app did. Summaries never include chat content.
pub fn log_action(conn: &Connection, category: &str, summary: &str) {
    let _ = conn.execute(
        "INSERT INTO action_log (at, category, summary) VALUES (?1, ?2, ?3)",
        params![now_ms(), category, summary],
    );
}

pub fn actions(conn: &Connection, limit: u32) -> Vec<Action> {
    let Ok(mut stmt) = conn.prepare("SELECT id, at, category, summary FROM action_log ORDER BY id DESC LIMIT ?1") else {
        return Vec::new();
    };
    stmt.query_map([limit], |r| Ok(Action { id: r.get(0)?, at: r.get(1)?, category: r.get(2)?, summary: r.get(3)? }))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn clear_actions(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM action_log", []).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Protector, Vault};

    struct Plain;
    impl Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }

    fn cipher() -> Cipher {
        let dir = std::env::temp_dir().join(format!("sulcusai-db-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap()
    }

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    fn msg(chat_id: &str, content: &str) -> Message {
        Message {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: chat_id.into(),
            role: "user".into(),
            content: content.into(),
            thinking: Some("hmm".into()),
            created_at: now_ms(),
        }
    }

    #[test]
    fn settings_default_to_offline_and_old_rows_gain_new_fields() {
        let conn = mem();
        let s = settings(&conn);
        assert_eq!(s.connectivity, Connectivity::Offline);
        assert!(!s.onboarded);
        assert_eq!(s.auto_lock_minutes, 15);
        // A settings row written before these fields existed still loads.
        set_raw(&conn, "settings", r#"{"connectivity":"web","default_model":null}"#).unwrap();
        let s = settings(&conn);
        assert_eq!(s.connectivity, Connectivity::Web);
        assert_eq!(s.auto_lock_minutes, 15);
    }

    #[test]
    fn chat_text_is_stored_encrypted_and_read_back_plain() {
        let conn = mem();
        let c = cipher();
        let chat = create_chat(&conn, &c, None).unwrap();
        set_chat_title(&conn, &c, &chat.id, "Tax questions").unwrap();
        add_message(&conn, &c, &msg(&chat.id, "my secret plan")).unwrap();
        set_profile(&conn, &c, &Profile { name: "Sam".into(), ..Default::default() }).unwrap();

        let raw_title: String = conn.query_row("SELECT title FROM chats", [], |r| r.get(0)).unwrap();
        let raw_msg: String = conn.query_row("SELECT content FROM messages", [], |r| r.get(0)).unwrap();
        let raw_profile = get_raw(&conn, "profile").unwrap();
        assert!(!raw_title.contains("Tax") && !raw_msg.contains("secret") && !raw_profile.contains("Sam"));

        assert_eq!(list_chats(&conn, &c)[0].title, "Tax questions");
        assert_eq!(messages(&conn, &c, &chat.id)[0].content, "my secret plan");
        assert_eq!(messages(&conn, &c, &chat.id)[0].thinking.as_deref(), Some("hmm"));
        assert_eq!(profile(&conn, &c).name, "Sam");
    }

    #[test]
    fn legacy_plaintext_is_encrypted_in_place() {
        let mut conn = mem();
        let c = cipher();
        conn.execute("INSERT INTO chats VALUES ('c1', 'Old title', NULL, 0, 1, 1)", []).unwrap();
        conn.execute("INSERT INTO messages VALUES ('m1', 'c1', 'user', 'old text', 'old thought', 1)", []).unwrap();
        set_raw(&conn, "profile", r#"{"name":"Old","about":"","preferences":""}"#).unwrap();

        assert_eq!(encrypt_legacy(&mut conn, &c).unwrap(), 3);
        assert_eq!(encrypt_legacy(&mut conn, &c).unwrap(), 0, "second pass changes nothing");
        let raw: String = conn.query_row("SELECT content FROM messages", [], |r| r.get(0)).unwrap();
        assert!(Cipher::is_encrypted(&raw));
        assert_eq!(messages(&conn, &c, "c1")[0].thinking.as_deref(), Some("old thought"));
        assert_eq!(list_chats(&conn, &c)[0].title, "Old title");
        assert_eq!(profile(&conn, &c).name, "Old");
    }

    #[test]
    fn deleting_a_chat_deletes_its_messages() {
        let conn = mem();
        let c = cipher();
        let chat = create_chat(&conn, &c, None).unwrap();
        add_message(&conn, &c, &msg(&chat.id, "hi")).unwrap();
        assert_eq!(message_count(&conn, &chat.id), 1);
        delete_chat(&conn, &chat.id).unwrap();
        assert_eq!(message_count(&conn, &chat.id), 0);
    }

    #[test]
    fn action_log_lists_newest_first() {
        let conn = mem();
        log_action(&conn, "model", "first");
        log_action(&conn, "network", "second");
        let a = actions(&conn, 10);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].summary, "second");
        clear_actions(&conn).unwrap();
        assert!(actions(&conn, 10).is_empty());
    }
}

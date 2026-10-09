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
CREATE TABLE IF NOT EXISTS folders (
  path     TEXT PRIMARY KEY,
  added_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS checkpoints (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id    TEXT NOT NULL,
  turn_id    TEXT NOT NULL,
  action     TEXT NOT NULL,
  path       TEXT NOT NULL,
  other      TEXT,
  backup     TEXT,
  created_at INTEGER NOT NULL,
  undone     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS checkpoints_by_turn ON checkpoints(turn_id);
CREATE TABLE IF NOT EXISTS memories (
  id          TEXT PRIMARY KEY,
  content     TEXT NOT NULL,
  project_id  TEXT,
  source_chat TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  instructions TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS connectors (
  id         TEXT PRIMARY KEY,
  data       TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS plugins (
  id         TEXT PRIMARY KEY,
  data       TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS schedules (
  id         TEXT PRIMARY KEY,
  data       TEXT NOT NULL,
  next_run   INTEGER,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS meetings (
  id         TEXT PRIMARY KEY,
  data       TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at   INTEGER,
  status     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS meeting_segments (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  speaker     TEXT NOT NULL,
  start       REAL NOT NULL,
  end         REAL NOT NULL,
  text        TEXT NOT NULL,
  translation TEXT
);
CREATE INDEX IF NOT EXISTS segments_by_meeting ON meeting_segments(meeting_id, start);
CREATE TABLE IF NOT EXISTS notes (
  id         TEXT PRIMARY KEY,
  data       TEXT NOT NULL,
  pinned     INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tasks (
  id           TEXT PRIMARY KEY,
  data         TEXT NOT NULL,
  due          INTEGER,
  due_has_time INTEGER NOT NULL DEFAULT 0,
  priority     INTEGER NOT NULL DEFAULT 0,
  remind_at    INTEGER,
  reminded     INTEGER NOT NULL DEFAULT 0,
  done_at      INTEGER,
  created_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mail_accounts (
  id         TEXT PRIMARY KEY,
  data       TEXT NOT NULL,
  synced_at  INTEGER,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mail_messages (
  id         TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES mail_accounts(id) ON DELETE CASCADE,
  folder     TEXT NOT NULL,
  uid        INTEGER NOT NULL,
  date       INTEGER NOT NULL,
  seen       INTEGER NOT NULL DEFAULT 0,
  data       TEXT NOT NULL,
  UNIQUE (account_id, folder, uid)
);
CREATE INDEX IF NOT EXISTS mail_by_date ON mail_messages(date DESC);
CREATE TABLE IF NOT EXISTS cal_accounts (
  id         TEXT PRIMARY KEY,
  data       TEXT NOT NULL,
  synced_at  INTEGER,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
  id         TEXT PRIMARY KEY,
  account_id TEXT REFERENCES cal_accounts(id) ON DELETE CASCADE,
  calendar   TEXT,
  start      INTEGER NOT NULL,
  end        INTEGER NOT NULL,
  all_day    INTEGER NOT NULL DEFAULT 0,
  data       TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS events_by_start ON events(start);
CREATE TABLE IF NOT EXISTS media (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,
  chat_id    TEXT,
  hidden     INTEGER NOT NULL DEFAULT 0,
  data       TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS media_by_time ON media(created_at);
CREATE TABLE IF NOT EXISTS project_folders (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  path       TEXT NOT NULL,
  PRIMARY KEY (project_id, path)
);
";

/// Columns added after the first release, created on older databases.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    ("messages", "tool_calls", "TEXT"),
    ("messages", "tool_call_id", "TEXT"),
    ("messages", "meta", "TEXT"),
    ("chats", "mode", "TEXT NOT NULL DEFAULT 'auto'"),
    ("chats", "project_id", "TEXT"),
    ("chats", "incognito", "INTEGER NOT NULL DEFAULT 0"),
    ("chats", "parent_id", "TEXT"),
    ("meeting_segments", "voice", "INTEGER"),
];

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else { return false };
    let names = stmt.query_map([], |r| r.get::<_, String>(1));
    names.map(|rows| rows.filter_map(Result::ok).any(|n| n == column)).unwrap_or(false)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    for (table, column, def) in ADDED_COLUMNS {
        if !has_column(conn, table, column) {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {def}"))?;
        }
    }
    Ok(())
}

/// An in-memory database with the app's tables, for tests.
#[cfg(test)]
pub fn init_for_test(conn: &Connection) {
    conn.execute_batch(SCHEMA).unwrap();
    migrate(conn).unwrap();
    crate::sync::track::install(conn).unwrap();
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // secure_delete overwrites deleted rows, so removed chats don't linger on disk.
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
    crate::sync::track::install(&conn)?;
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
    /// Remember things across chats.
    #[serde(default = "default_true")]
    pub memory_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            connectivity: Connectivity::Offline,
            default_model: None,
            onboarded: false,
            auto_lock_minutes: default_auto_lock(),
            memory_enabled: true,
        }
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
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub about: String,
    pub preferences: String,
    /// Where they are, e.g. "Orem, Utah" (weather and local questions).
    pub location: String,
    /// Set by “Use this PC's location”.
    pub lat: Option<f64>,
    pub lon: Option<f64>,
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
    /// Run mode: "plan", "auto" or "bypass".
    pub mode: String,
    pub project_id: Option<String>,
    /// Doesn't use or create memories, and is deleted when you leave it.
    pub incognito: bool,
    /// The chat this one continues (auto-handoff).
    pub parent_id: Option<String>,
    /// No messages yet: a new chat stays out of the sidebar until the
    /// first message is sent.
    #[serde(default)]
    pub empty: bool,
}

const CHAT_COLS: &str = "id, title, model_id, web, created_at, updated_at, mode, project_id, incognito, parent_id";

fn chat_from_row(c: &Cipher) -> impl Fn(&rusqlite::Row) -> rusqlite::Result<Chat> + '_ {
    move |r| {
        Ok(Chat {
            id: r.get(0)?,
            title: c.decrypt_or(&r.get::<_, String>(1)?, "(unreadable chat)"),
            model_id: r.get(2)?,
            web: r.get::<_, i64>(3)? != 0,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
            mode: r.get(6)?,
            project_id: r.get(7)?,
            incognito: r.get::<_, i64>(8)? != 0,
            parent_id: r.get(9)?,
            empty: false,
        })
    }
}

/// Incognito chats don't outlive the session.
pub fn delete_incognito_chats(conn: &Connection, except: Option<&str>) -> Result<usize, String> {
    conn.execute("DELETE FROM chats WHERE incognito = 1 AND id != ?1", [except.unwrap_or("")]).map_err(err)
}

pub fn set_chat_parent(conn: &Connection, id: &str, parent: &str) -> Result<(), String> {
    conn.execute("UPDATE chats SET parent_id = ?2 WHERE id = ?1", params![id, parent]).map_err(err)?;
    Ok(())
}

pub fn set_chat_mode(conn: &Connection, id: &str, mode: &str) -> Result<(), String> {
    if !matches!(mode, "plan" | "auto" | "bypass") {
        return Err("Unknown mode.".into());
    }
    conn.execute("UPDATE chats SET mode = ?2 WHERE id = ?1", params![id, mode]).map_err(err)?;
    Ok(())
}

pub fn list_chats(conn: &Connection, c: &Cipher) -> Vec<Chat> {
    let sql = format!("SELECT {CHAT_COLS} FROM chats ORDER BY updated_at DESC");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    let mut chats: Vec<Chat> = stmt
        .query_map([], chat_from_row(c))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();
    let used: std::collections::HashSet<String> = conn
        .prepare("SELECT DISTINCT chat_id FROM messages")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    for chat in &mut chats {
        chat.empty = !used.contains(&chat.id);
    }
    chats
}

/// Drops new chats nobody wrote in (they never showed in the sidebar).
pub fn delete_empty_chats(conn: &Connection) -> Result<usize, String> {
    conn.execute("DELETE FROM chats WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.chat_id = chats.id)", [])
        .map_err(|e| e.to_string())
}

pub fn chat(conn: &Connection, c: &Cipher, id: &str) -> Option<Chat> {
    let sql = format!("SELECT {CHAT_COLS} FROM chats WHERE id = ?1");
    conn.query_row(&sql, [id], chat_from_row(c)).optional().ok().flatten()
}

pub fn create_chat(conn: &Connection, c: &Cipher, model_id: Option<String>) -> Result<Chat, String> {
    create_chat_in(conn, c, model_id, None, false)
}

pub fn create_chat_in(conn: &Connection, c: &Cipher, model_id: Option<String>, project_id: Option<String>, incognito: bool) -> Result<Chat, String> {
    let now = now_ms();
    let chat = Chat {
        id: uuid::Uuid::new_v4().to_string(),
        title: if incognito { "Incognito chat".into() } else { "New chat".into() },
        model_id,
        web: false,
        created_at: now,
        updated_at: now,
        mode: "auto".into(),
        project_id,
        incognito,
        parent_id: None,
        empty: true,
    };
    conn.execute(
        "INSERT INTO chats (id, title, model_id, web, created_at, updated_at, project_id, incognito) VALUES (?1, ?2, ?3, 0, ?4, ?4, ?5, ?6)",
        params![chat.id, c.encrypt(&chat.title), chat.model_id, now, chat.project_id, incognito as i64],
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

/// A tool call the model made, as sent back to it in later turns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON text, exactly as the model produced it.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    /// "user", "assistant" or "tool".
    pub role: String,
    pub content: String,
    pub thinking: Option<String>,
    pub created_at: i64,
    /// Assistant messages: tools it asked to run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Tool messages: which call this answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool messages: what the window shows (title, status, diff, output).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

pub fn messages(conn: &Connection, c: &Cipher, chat_id: &str) -> Vec<Message> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, chat_id, role, content, thinking, created_at, tool_calls, tool_call_id, meta
         FROM messages WHERE chat_id = ?1 ORDER BY created_at, rowid",
    ) else {
        return Vec::new();
    };
    let decrypt_json = |raw: Option<String>| -> Option<String> { raw.and_then(|t| c.decrypt(&t).ok()) };
    stmt.query_map([chat_id], |r| {
        Ok(Message {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            role: r.get(2)?,
            content: c.decrypt_or(&r.get::<_, String>(3)?, "(This message couldn't be decrypted.)"),
            thinking: r.get::<_, Option<String>>(4)?.map(|t| c.decrypt_or(&t, "")),
            created_at: r.get(5)?,
            tool_calls: decrypt_json(r.get(6)?).and_then(|j| serde_json::from_str(&j).ok()),
            tool_call_id: r.get(7)?,
            meta: decrypt_json(r.get(8)?).and_then(|j| serde_json::from_str(&j).ok()),
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
    let enc_json = |v: Option<String>| v.map(|j| c.encrypt(&j));
    let tool_calls = enc_json(m.tool_calls.as_ref().and_then(|t| serde_json::to_string(t).ok()));
    let meta = enc_json(m.meta.as_ref().map(|v| v.to_string()));
    conn.execute(
        "INSERT INTO messages (id, chat_id, role, content, thinking, created_at, tool_calls, tool_call_id, meta)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            m.id,
            m.chat_id,
            m.role,
            c.encrypt(&m.content),
            m.thinking.as_deref().map(|t| c.encrypt(t)),
            m.created_at,
            tool_calls,
            m.tool_call_id,
            meta
        ],
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

// ---------- shared folders ----------

pub fn folders(conn: &Connection) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT path FROM folders ORDER BY added_at") else { return Vec::new() };
    stmt.query_map([], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn add_folder(conn: &Connection, path: &str) -> Result<(), String> {
    conn.execute("INSERT OR IGNORE INTO folders (path, added_at) VALUES (?1, ?2)", params![path, now_ms()]).map_err(err)?;
    Ok(())
}

pub fn remove_folder(conn: &Connection, path: &str) -> Result<(), String> {
    conn.execute("DELETE FROM folders WHERE path = ?1", [path]).map_err(err)?;
    Ok(())
}

// ---------- checkpoints (undo for agent file changes) ----------

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub id: i64,
    pub action: String,
    pub path: String,
    pub other: Option<String>,
    pub backup: Option<String>,
}

pub fn add_checkpoint(
    conn: &Connection,
    c: &Cipher,
    chat_id: &str,
    turn_id: &str,
    action: &str,
    path: &str,
    other: Option<&str>,
    backup: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO checkpoints (chat_id, turn_id, action, path, other, backup, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![chat_id, turn_id, action, c.encrypt(path), other.map(|o| c.encrypt(o)), backup, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// Not-yet-undone changes from one turn, newest first (the order to undo them in).
pub fn turn_checkpoints(conn: &Connection, c: &Cipher, turn_id: &str) -> Vec<Checkpoint> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, action, path, other, backup FROM checkpoints WHERE turn_id = ?1 AND undone = 0 ORDER BY id DESC",
    ) else {
        return Vec::new();
    };
    stmt.query_map([turn_id], |r| {
        Ok(Checkpoint {
            id: r.get(0)?,
            action: r.get(1)?,
            path: c.decrypt_or(&r.get::<_, String>(2)?, ""),
            other: r.get::<_, Option<String>>(3)?.map(|o| c.decrypt_or(&o, "")),
            backup: r.get(4)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Turn ids in this chat that still have changes to undo.
pub fn undoable_turns(conn: &Connection, chat_id: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT DISTINCT turn_id FROM checkpoints WHERE chat_id = ?1 AND undone = 0") else {
        return Vec::new();
    };
    stmt.query_map([chat_id], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn mark_undone(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("UPDATE checkpoints SET undone = 1 WHERE id = ?1", [id]).map_err(err)?;
    Ok(())
}

/// Backups older than `days`, for cleanup. Returns (id, backup path) pairs.
pub fn old_checkpoints(conn: &Connection, days: i64) -> Vec<(i64, Option<String>)> {
    let cutoff = now_ms() - days * 86_400_000;
    let Ok(mut stmt) = conn.prepare("SELECT id, backup FROM checkpoints WHERE created_at < ?1") else { return Vec::new() };
    stmt.query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn delete_checkpoint(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM checkpoints WHERE id = ?1", [id]).map_err(err)?;
    Ok(())
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

/// For entries that name the user's files (agent actions).
pub fn log_action_enc(conn: &Connection, c: &Cipher, category: &str, summary: &str) {
    log_action(conn, category, &c.encrypt(summary));
}

pub fn actions(conn: &Connection, c: &Cipher, limit: u32) -> Vec<Action> {
    let Ok(mut stmt) = conn.prepare("SELECT id, at, category, summary FROM action_log ORDER BY id DESC LIMIT ?1") else {
        return Vec::new();
    };
    stmt.query_map([limit], |r| {
        Ok(Action { id: r.get(0)?, at: r.get(1)?, category: r.get(2)?, summary: c.decrypt_or(&r.get::<_, String>(3)?, "(unreadable)") })
    })
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
        migrate(&conn).unwrap();
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
            ..Default::default()
        }
    }

    #[test]
    fn old_databases_gain_the_new_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE chats (id TEXT PRIMARY KEY, title TEXT NOT NULL, model_id TEXT, web INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             CREATE TABLE messages (id TEXT PRIMARY KEY, chat_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL, thinking TEXT, created_at INTEGER NOT NULL);
             INSERT INTO chats VALUES ('c', 't', NULL, 0, 1, 1);",
        )
        .unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent
        let mode: String = conn.query_row("SELECT mode FROM chats", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "auto");
        assert!(has_column(&conn, "messages", "tool_calls"));
    }

    #[test]
    fn tool_calls_and_results_round_trip_encrypted() {
        let conn = mem();
        let c = cipher();
        let chat = create_chat(&conn, &c, None).unwrap();
        let call = ToolCall { id: "call_1".into(), name: "read_file".into(), arguments: r#"{"path":"secret-plan.txt"}"#.into() };
        add_message(&conn, &c, &Message {
            id: "a1".into(),
            chat_id: chat.id.clone(),
            role: "assistant".into(),
            tool_calls: Some(vec![call.clone()]),
            created_at: 1,
            ..Default::default()
        })
        .unwrap();
        add_message(&conn, &c, &Message {
            id: "t1".into(),
            chat_id: chat.id.clone(),
            role: "tool".into(),
            content: "file text".into(),
            tool_call_id: Some("call_1".into()),
            meta: Some(serde_json::json!({ "title": "Read secret-plan.txt" })),
            created_at: 2,
            ..Default::default()
        })
        .unwrap();
        let raw: String = conn.query_row("SELECT tool_calls FROM messages WHERE id = 'a1'", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("secret-plan"));
        let msgs = messages(&conn, &c, &chat.id);
        assert_eq!(msgs[0].tool_calls.as_ref().unwrap()[0], call);
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(msgs[1].meta.as_ref().unwrap()["title"], "Read secret-plan.txt");
    }

    #[test]
    fn checkpoints_list_newest_first_and_can_be_marked_undone() {
        let conn = mem();
        let c = cipher();
        add_checkpoint(&conn, &c, "chat", "turn", "create", "a.txt", None, None).unwrap();
        add_checkpoint(&conn, &c, "chat", "turn", "modify", "b.txt", None, Some("bk")).unwrap();
        let cps = turn_checkpoints(&conn, &c, "turn");
        assert_eq!(cps.len(), 2);
        assert_eq!(cps[0].path, "b.txt");
        assert_eq!(undoable_turns(&conn, "chat"), vec!["turn".to_string()]);
        for cp in &cps {
            mark_undone(&conn, cp.id).unwrap();
        }
        assert!(undoable_turns(&conn, "chat").is_empty());
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
        conn.execute("INSERT INTO chats (id, title, model_id, web, created_at, updated_at) VALUES ('c1', 'Old title', NULL, 0, 1, 1)", []).unwrap();
        conn.execute(
            "INSERT INTO messages (id, chat_id, role, content, thinking, created_at) VALUES ('m1', 'c1', 'user', 'old text', 'old thought', 1)",
            [],
        )
        .unwrap();
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
        let c = cipher();
        log_action_enc(&conn, &c, "agent", "Edited secret.txt");
        let a = actions(&conn, &c, 10);
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].summary, "Edited secret.txt", "decrypted for display");
        assert_eq!(a[1].summary, "second");
        let raw: String = conn.query_row("SELECT summary FROM action_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("secret"), "file names are encrypted at rest");
        clear_actions(&conn).unwrap();
        assert!(actions(&conn, &c, 10).is_empty());
    }
}

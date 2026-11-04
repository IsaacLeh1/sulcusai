// SPDX-License-Identifier: AGPL-3.0-only
//! Personal life: records found in mail, plans and people, health and home,
//! and the mind pages. Every record is one encrypted JSON row; only its kind
//! and the date it sorts by stay plain, so lists sort without decrypting.

pub mod home;
pub mod mail_records;
pub mod mind;
pub mod plans;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::Cipher;
use crate::db;
use crate::AppStateRef;

const SWITCH: &str = "personal_enabled";

/// The kinds a record can be. Unknown kinds are refused so a typo in the
/// window can't create rows no page ever shows.
pub const KINDS: &[&str] = &[
    "package", "booking", "event", "newsletter", "birthday", "application", "trip", "contact", "family",
    "food", "workout", "health", "recipe", "meal_plan", "car", "plant", "pet", "journal", "meditation",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub id: String,
    pub kind: String,
    /// Milliseconds; the date the record is about (delivery day, trip start,
    /// birthday, entry day).
    pub at: i64,
    pub data: Value,
    pub updated_at: i64,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn ensure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS personal_records (
           id TEXT PRIMARY KEY, kind TEXT NOT NULL, at INTEGER NOT NULL,
           data TEXT NOT NULL, updated_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS personal_by_kind ON personal_records(kind, at);",
    )
    .map_err(err)
}

pub fn save(conn: &Connection, c: &Cipher, id: Option<&str>, kind: &str, at: i64, data: &Value) -> Result<Record, String> {
    if !KINDS.contains(&kind) {
        return Err(format!("Unknown record kind: {kind}"));
    }
    ensure(conn)?;
    let id = id.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = db::now_ms();
    conn.execute(
        "INSERT INTO personal_records (id, kind, at, data, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, at = excluded.at, data = excluded.data, updated_at = excluded.updated_at",
        params![id, kind, at, c.encrypt(&data.to_string()), now],
    )
    .map_err(err)?;
    Ok(Record { id, kind: kind.into(), at, data: data.clone(), updated_at: now })
}

pub fn list(conn: &Connection, c: &Cipher, kind: &str) -> Vec<Record> {
    if ensure(conn).is_err() {
        return Vec::new();
    }
    let Ok(mut stmt) = conn.prepare("SELECT id, kind, at, data, updated_at FROM personal_records WHERE kind = ?1 ORDER BY at DESC") else {
        return Vec::new();
    };
    stmt.query_map([kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .map(|(id, kind, at, data, updated_at)| Record {
                    id,
                    kind,
                    at,
                    data: c.decrypt(&data).ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(Value::Null),
                    updated_at,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn delete(conn: &Connection, id: &str) -> Result<(), String> {
    ensure(conn)?;
    conn.execute("DELETE FROM personal_records WHERE id = ?1", [id]).map_err(err)?;
    Ok(())
}

pub fn enabled(conn: &Connection) -> bool {
    db::get::<bool>(conn, SWITCH).unwrap_or(false)
}

fn require(conn: &Connection) -> Result<(), String> {
    if enabled(conn) {
        Ok(())
    } else {
        Err("Personal life is turned off in Settings.".into())
    }
}

// ---------- commands ----------

#[tauri::command(async)]
pub fn personal_enabled(state: AppStateRef) -> bool {
    enabled(&state.db.lock().unwrap())
}

#[tauri::command(async)]
pub fn personal_set_enabled(state: AppStateRef, on: bool) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), SWITCH, &on)
}

#[tauri::command(async)]
pub fn personal_list(state: AppStateRef, kind: String) -> Result<Vec<Record>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn)?;
    Ok(list(&conn, &c, &kind))
}

#[tauri::command(async)]
pub fn personal_save(state: AppStateRef, id: Option<String>, kind: String, at: i64, data: Value) -> Result<Record, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn)?;
    save(&conn, &c, id.as_deref(), &kind, at, &data)
}

#[tauri::command(async)]
pub fn personal_delete(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn)?;
    delete(&conn, &id)?;
    db::log_action(&conn, "personal", "Deleted a personal record");
    Ok(())
}

/// Reads one message and, when it is a package, booking, event, newsletter,
/// birthday or application, saves it as a record. Nothing is saved for
/// ordinary mail.
#[tauri::command(async)]
pub fn personal_from_mail(state: AppStateRef, subject: String, from: String, body: String, received_ms: i64) -> Result<Option<Record>, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn)?;
    let Some(found) = mail_records::read(&subject, &from, &body, received_ms) else {
        return Ok(None);
    };
    save(&conn, &c, None, found.kind, found.at, &found.data).map(Some)
}

#[tauri::command(async)]
pub fn personal_tasks_from_text(text: String) -> Vec<mind::LooseTask> {
    mind::tasks_from_rambling(&text, chrono::Local::now().date_naive())
}

#[tauri::command(async)]
pub fn personal_profile_check(text: String) -> Vec<String> {
    mind::profile_tips(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        c
    }

    #[test]
    fn records_round_trip_encrypted() {
        let conn = conn();
        let c = Cipher::random();
        let r = save(&conn, &c, None, "plant", 5, &json!({"name": "Fern"})).unwrap();
        let raw: String = conn.query_row("SELECT data FROM personal_records", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("Fern"));
        let got = list(&conn, &c, "plant");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data["name"], "Fern");
        save(&conn, &c, Some(&r.id), "plant", 6, &json!({"name": "Ivy"})).unwrap();
        assert_eq!(list(&conn, &c, "plant")[0].data["name"], "Ivy");
        delete(&conn, &r.id).unwrap();
        assert!(list(&conn, &c, "plant").is_empty());
    }

    #[test]
    fn unknown_kinds_refused_and_switch_starts_off() {
        let conn = conn();
        assert!(save(&conn, &Cipher::random(), None, "nope", 0, &json!({})).is_err());
        assert!(!enabled(&conn));
        db::set(&conn, SWITCH, &true).unwrap();
        assert!(enabled(&conn));
    }
}

/// chrono is built without serde here, so dates go out as YYYY-MM-DD text.
pub(crate) fn date_text<S: serde::Serializer>(d: &chrono::NaiveDate, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&d.to_string())
}

pub(crate) fn opt_date_text<S: serde::Serializer>(d: &Option<chrono::NaiveDate>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_str(&d.to_string()),
        None => s.serialize_none(),
    }
}

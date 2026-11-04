// SPDX-License-Identifier: AGPL-3.0-only
//! One encrypted records store shared by the home inventory, filled-in forms
//! and scans. Each record is a JSON object sealed with the app's data key, so
//! the file on disk shows only kinds and times. Lists are small (hundreds,
//! not millions), so searching opens them in memory.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::Cipher;

pub const KINDS: &[&str] = &["inventory", "form", "scan"];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,
    pub kind: String,
    pub created: i64,
    pub updated: i64,
    pub data: Value,
}

pub struct Records {
    db: Connection,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Records {
    pub fn open(path: &Path) -> rusqlite::Result<Records> {
        Self::setup(Connection::open(path)?)
    }

    pub fn in_memory() -> rusqlite::Result<Records> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(db: Connection) -> rusqlite::Result<Records> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS records (
                 id TEXT PRIMARY KEY, kind TEXT NOT NULL, created INTEGER NOT NULL,
                 updated INTEGER NOT NULL, sealed BLOB NOT NULL);
             CREATE INDEX IF NOT EXISTS records_kind ON records(kind);",
        )?;
        Ok(Records { db })
    }

    pub fn put(&self, c: &Cipher, kind: &str, id: Option<&str>, data: &Value) -> Result<Record, String> {
        if !KINDS.contains(&kind) {
            return Err(format!("Unknown record kind: {kind}"));
        }
        if !data.is_object() {
            return Err("A record is a set of named fields.".into());
        }
        let sealed = c.seal_bytes(serde_json::to_vec(data).map_err(|e| e.to_string())?.as_slice());
        let t = now();
        let id = match id {
            Some(id) => {
                let n = self
                    .db
                    .execute("UPDATE records SET sealed=?3, updated=?4 WHERE id=?1 AND kind=?2", params![id, kind, sealed, t])
                    .map_err(|e| e.to_string())?;
                if n == 0 {
                    return Err("That record is gone.".into());
                }
                id.to_string()
            }
            None => {
                let id = uuid::Uuid::new_v4().simple().to_string();
                self.db
                    .execute("INSERT INTO records(id, kind, created, updated, sealed) VALUES (?1, ?2, ?3, ?3, ?4)", params![id, kind, t, sealed])
                    .map_err(|e| e.to_string())?;
                id
            }
        };
        self.get(c, &id)?.ok_or_else(|| "Saving failed.".into())
    }

    pub fn get(&self, c: &Cipher, id: &str) -> Result<Option<Record>, String> {
        let row: Option<(String, i64, i64, Vec<u8>)> = self
            .db
            .query_row("SELECT kind, created, updated, sealed FROM records WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        row.map(|(kind, created, updated, sealed)| open(c, id.to_string(), kind, created, updated, &sealed)).transpose()
    }

    pub fn list(&self, c: &Cipher, kind: &str) -> Result<Vec<Record>, String> {
        let mut st = self.db.prepare("SELECT id, created, updated, sealed FROM records WHERE kind=?1 ORDER BY updated DESC").map_err(|e| e.to_string())?;
        let rows: Vec<(String, i64, i64, Vec<u8>)> =
            st.query_map([kind], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        rows.into_iter().map(|(id, cr, up, s)| open(c, id, kind.to_string(), cr, up, &s)).collect()
    }

    /// Records of a kind whose any field contains every word (any case).
    pub fn search(&self, c: &Cipher, kind: &str, q: &str) -> Result<Vec<Record>, String> {
        let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
        Ok(self
            .list(c, kind)?
            .into_iter()
            .filter(|r| {
                let text = r.data.to_string().to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .collect())
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        Ok(self.db.execute("DELETE FROM records WHERE id=?1", [id]).map_err(|e| e.to_string())? > 0)
    }
}

fn open(c: &Cipher, id: String, kind: String, created: i64, updated: i64, sealed: &[u8]) -> Result<Record, String> {
    let plain = c.open_bytes(sealed)?;
    let data = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
    Ok(Record { id, kind, created, updated, data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_are_sealed_and_round_trip() {
        let c = Cipher::for_test();
        let r = Records::in_memory().unwrap();
        let a = r.put(&c, "inventory", None, &json!({"name": "Guitar", "serial": "XJ-77"})).unwrap();
        let raw: Vec<u8> = r.db.query_row("SELECT sealed FROM records", [], |x| x.get(0)).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("XJ-77"), "nothing readable on disk");
        assert_eq!(r.get(&c, &a.id).unwrap().unwrap().data["serial"], "XJ-77");
        let b = r.put(&c, "inventory", Some(&a.id), &json!({"name": "Guitar", "serial": "XJ-78"})).unwrap();
        assert_eq!(b.id, a.id);
        assert_eq!(r.search(&c, "inventory", "xj-78").unwrap().len(), 1);
        assert!(r.search(&c, "form", "xj").unwrap().is_empty());
        assert!(r.put(&c, "nonsense", None, &json!({})).is_err());
        assert!(r.put(&c, "inventory", None, &json!("text")).is_err());
        let other = Cipher::random();
        assert!(r.get(&other, &a.id).is_err(), "another key can't open it");
        assert!(r.delete(&a.id).unwrap());
        assert!(r.put(&c, "inventory", Some(&a.id), &json!({})).is_err());
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Which rows changed, and applying rows from another PC.
//!
//! Triggers on each synced table note every change in `sync_state` with a
//! local sequence number (what another PC has already been sent) and a time
//! (which copy wins when two PCs changed the same item: the later one, item
//! by item). Incognito chats never get a row; a chat that leaves incognito
//! brings its messages along.

use std::collections::{BTreeMap, HashMap};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::crypto::Cipher;

pub struct Table {
    pub name: &'static str,
    key: &'static str,
    /// The time an existing row last changed, for the first count.
    stamp: &'static str,
    /// Which rows sync (trigger conditions on NEW / OLD, and the first count's filter).
    when_new: &'static str,
    when_old: &'static str,
    filter: &'static str,
}

/// In the order rows are applied (a chat before its messages).
pub const TABLES: &[Table] = &[
    Table { name: "projects", key: "id", stamp: "updated_at", when_new: "", when_old: "", filter: "" },
    Table {
        name: "chats",
        key: "id",
        stamp: "updated_at",
        when_new: "WHEN NEW.incognito = 0",
        when_old: "WHEN OLD.incognito = 0",
        filter: "WHERE incognito = 0",
    },
    Table {
        name: "messages",
        key: "id",
        stamp: "created_at",
        when_new: "WHEN (SELECT incognito FROM chats WHERE id = NEW.chat_id) = 0",
        when_old: "WHEN (SELECT incognito FROM chats WHERE id = OLD.chat_id) = 0",
        filter: "WHERE chat_id IN (SELECT id FROM chats WHERE incognito = 0)",
    },
    Table { name: "memories", key: "id", stamp: "updated_at", when_new: "", when_old: "", filter: "" },
    Table { name: "notes", key: "id", stamp: "updated_at", when_new: "", when_old: "", filter: "" },
    Table { name: "tasks", key: "id", stamp: "created_at", when_new: "", when_old: "", filter: "" },
    // Only "About you"; every other setting belongs to its PC.
    Table {
        name: "settings",
        key: "key",
        stamp: "0",
        when_new: "WHEN NEW.key IN ('profile')",
        when_old: "WHEN OLD.key IN ('profile')",
        filter: "WHERE key IN ('profile')",
    },
];
const SETTING_KEYS: &[&str] = &["profile"];

const NOW: &str = "CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)";

fn mark(table: &str, id: &str, deleted: u8) -> String {
    // A PC whose clock went back still moves its own edits forward.
    format!(
        "INSERT INTO sync_state (tbl, id, seq, changed_at, deleted) VALUES ('{table}', {id}, (SELECT IFNULL(MAX(seq), 0) + 1 FROM sync_state), {NOW}, {deleted})
         ON CONFLICT (tbl, id) DO UPDATE SET seq = excluded.seq, changed_at = MAX(excluded.changed_at, sync_state.changed_at + 1),
         deleted = excluded.deleted, origin = NULL, via = NULL;"
    )
}

/// Creates the change list and its triggers; the first time, counts every
/// existing row as changed when it was last edited.
pub fn install(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (
           tbl        TEXT NOT NULL,
           id         TEXT NOT NULL,
           seq        INTEGER NOT NULL,
           changed_at INTEGER NOT NULL,
           deleted    INTEGER NOT NULL DEFAULT 0,
           origin     TEXT,
           via        TEXT,
           PRIMARY KEY (tbl, id)
         );
         CREATE INDEX IF NOT EXISTS sync_by_seq ON sync_state(seq);",
    )?;
    let empty: bool = conn.query_row("SELECT NOT EXISTS (SELECT 1 FROM sync_state)", [], |r| r.get(0))?;
    if empty {
        for t in TABLES {
            conn.execute_batch(&format!(
                "INSERT OR IGNORE INTO sync_state (tbl, id, seq, changed_at, deleted) SELECT '{}', {}, 1, {}, 0 FROM {} {};",
                t.name, t.key, t.stamp, t.name, t.filter
            ))?;
        }
    }
    let mut sql = String::new();
    for t in TABLES {
        let (name, new, old) = (t.name, format!("NEW.{}", t.key), format!("OLD.{}", t.key));
        sql += &format!(
            "CREATE TRIGGER IF NOT EXISTS sync1_{name}_i AFTER INSERT ON {name} {} BEGIN {} END;
             CREATE TRIGGER IF NOT EXISTS sync1_{name}_u AFTER UPDATE ON {name} {} BEGIN {} END;
             CREATE TRIGGER IF NOT EXISTS sync1_{name}_d AFTER DELETE ON {name} {} BEGIN {} END;\n",
            t.when_new,
            mark(name, &new, 0),
            t.when_new,
            mark(name, &new, 0),
            t.when_old,
            mark(name, &old, 1),
        );
    }
    sql += &format!(
        "CREATE TRIGGER IF NOT EXISTS sync1_chats_unhide AFTER UPDATE OF incognito ON chats WHEN OLD.incognito = 1 AND NEW.incognito = 0 BEGIN
           INSERT OR IGNORE INTO sync_state (tbl, id, seq, changed_at, deleted)
           SELECT 'messages', id, (SELECT IFNULL(MAX(seq), 0) + 1 FROM sync_state), {NOW}, 0 FROM messages WHERE chat_id = NEW.id;
         END;"
    );
    conn.execute_batch(&sql)
}

/// One column's value on the wire. Text this PC stores encrypted travels as
/// plain text inside the encrypted link and is sealed again with the other
/// PC's own key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Val {
    N,
    I(i64),
    F(f64),
    T(String),
    S(String),
    B(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub t: String,
    pub id: String,
    /// When it changed, and on which PC (for the later-one-wins rule).
    pub at: i64,
    pub by: String,
    #[serde(default)]
    pub del: bool,
    #[serde(default)]
    pub row: Option<BTreeMap<String, Val>>,
}

/// Where a batch ended, so the next one starts after it.
pub type Mark = (i64, String, String);

pub fn max_seq(conn: &Connection) -> i64 {
    conn.query_row("SELECT IFNULL(MAX(seq), 0) FROM sync_state", [], |r| r.get(0)).unwrap_or(0)
}

/// Anything changed here after `since` that `peer` didn't send us.
pub fn has_news(conn: &Connection, since: i64, peer: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sync_state WHERE seq > ?1 AND (via IS NULL OR via != ?2))",
        params![since, peer],
        |r| r.get(0),
    )
    .unwrap_or(false)
}

fn table(name: &str) -> Option<&'static Table> {
    TABLES.iter().find(|t| t.name == name)
}

fn read_row(conn: &Connection, c: &Cipher, t: &Table, id: &str) -> Result<Option<BTreeMap<String, Val>>, String> {
    let mut stmt = conn.prepare_cached(&format!("SELECT * FROM {} WHERE {} = ?1", t.name, t.key)).map_err(|e| e.to_string())?;
    let names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    let row = stmt
        .query_row([id], |r| (0..names.len()).map(|i| r.get::<_, Value>(i)).collect::<rusqlite::Result<Vec<_>>>())
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(values) = row else { return Ok(None) };
    let mut out = BTreeMap::new();
    for (name, v) in names.into_iter().zip(values) {
        let v = match v {
            Value::Null => Val::N,
            Value::Integer(i) => Val::I(i),
            Value::Real(f) => Val::F(f),
            Value::Text(s) if Cipher::is_encrypted(&s) => Val::S(c.decrypt(&s)?),
            Value::Text(s) => Val::T(s),
            Value::Blob(b) => Val::B(B64.encode(b)),
        };
        out.insert(name, v);
    }
    Ok(Some(out))
}

/// Up to `limit` changes after `after` (and no later than `up_to`) that
/// didn't come from `peer`, with where the batch ended.
pub fn changes(conn: &Connection, c: &Cipher, me: &str, peer: &str, after: &Mark, up_to: i64, limit: usize) -> Result<(Vec<Change>, Option<Mark>), String> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT tbl, id, seq, changed_at, deleted, origin, via FROM sync_state
             WHERE (seq, tbl, id) > (?1, ?2, ?3) AND seq <= ?4
             ORDER BY seq, tbl, id LIMIT ?5",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![after.0, after.1, after.2, up_to, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let end = rows.last().map(|r| (r.2, r.0.clone(), r.1.clone()));
    let mut out = Vec::new();
    for (tbl, id, _, at, deleted, origin, via) in rows {
        if via.as_deref() == Some(peer) {
            continue;
        }
        let Some(t) = table(&tbl) else { continue };
        let row = if deleted {
            None
        } else {
            match read_row(conn, c, t, &id) {
                Ok(Some(r)) => Some(r),
                // Gone since (its delete is a later change) or unreadable here.
                _ => continue,
            }
        };
        out.push(Change { t: tbl, id, at, by: origin.unwrap_or_else(|| me.to_string()), del: deleted, row });
    }
    Ok((out, end))
}

/// The columns this PC's copy of a table has.
#[derive(Default)]
pub struct Columns(HashMap<&'static str, Vec<String>>);

impl Columns {
    fn of(&mut self, conn: &Connection, t: &'static Table) -> &[String] {
        self.0.entry(t.name).or_insert_with(|| {
            conn.prepare(&format!("PRAGMA table_info({})", t.name))
                .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1))?.collect())
                .unwrap_or_default()
        })
    }
}

/// Applies changes from `peer` where they're newer than this PC's copy.
/// Returns how many were taken, and the ones that couldn't be yet (a
/// message that came before its chat): try those again at the end.
pub fn apply(conn: &Connection, c: &Cipher, me: &str, peer: &str, items: &[Change], cols: &mut Columns) -> Result<(usize, Vec<Change>), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut taken = 0;
    let mut later = Vec::new();
    for item in items {
        let Some(t) = table(&item.t) else { continue };
        if t.name == "settings" && !SETTING_KEYS.contains(&item.id.as_str()) {
            continue;
        }
        let local: Option<(i64, Option<String>)> = tx
            .query_row("SELECT changed_at, origin FROM sync_state WHERE tbl = ?1 AND id = ?2", params![t.name, item.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((at, origin)) = &local {
            let by = origin.as_deref().unwrap_or(me);
            if (item.at, item.by.as_str()) <= (*at, by) {
                continue;
            }
        }
        let done = if item.del {
            tx.execute(&format!("DELETE FROM {} WHERE {} = ?1", t.name, t.key), [&item.id]).map(|_| ())
        } else {
            let Some(row) = &item.row else { continue };
            let have = cols.of(&tx, t);
            let mut names = Vec::new();
            let mut values = Vec::new();
            for (k, v) in row {
                if !have.contains(k) {
                    continue;
                }
                names.push(k.clone());
                values.push(match v {
                    Val::N => Value::Null,
                    Val::I(i) => Value::Integer(*i),
                    Val::F(f) => Value::Real(*f),
                    Val::T(s) => Value::Text(s.clone()),
                    Val::S(s) => Value::Text(c.encrypt(s)),
                    Val::B(b) => Value::Blob(B64.decode(b).unwrap_or_default()),
                });
            }
            if !names.iter().any(|n| n == t.key) {
                continue;
            }
            let marks: Vec<String> = (1..=names.len()).map(|i| format!("?{i}")).collect();
            let sets: Vec<String> = names.iter().filter(|n| *n != t.key).map(|n| format!("{n} = excluded.{n}")).collect();
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT({}) DO {}",
                t.name,
                names.join(", "),
                marks.join(", "),
                t.key,
                if sets.is_empty() { "NOTHING".to_string() } else { format!("UPDATE SET {}", sets.join(", ")) }
            );
            tx.execute(&sql, rusqlite::params_from_iter(values)).map(|_| ())
        };
        if done.is_err() {
            later.push(item.clone());
            continue;
        }
        let origin = (item.by != me).then_some(item.by.as_str());
        tx.execute(
            "INSERT INTO sync_state (tbl, id, seq, changed_at, deleted, origin, via)
             VALUES (?1, ?2, (SELECT IFNULL(MAX(seq), 0) + 1 FROM sync_state), ?3, ?4, ?5, ?6)
             ON CONFLICT (tbl, id) DO UPDATE SET changed_at = excluded.changed_at, deleted = excluded.deleted, origin = excluded.origin, via = excluded.via",
            params![t.name, item.id, item.at, item.del, origin, peer],
        )
        .map_err(|e| e.to_string())?;
        taken += 1;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok((taken, later))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc() -> (Connection, Cipher) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::init_for_test(&conn);
        (conn, Cipher::for_test())
    }

    /// Everything `from` has that `to` hasn't seen, as one session would send it.
    fn send(from: (&Connection, &Cipher, &str), to: (&Connection, &Cipher, &str), since: i64) -> (usize, i64) {
        let up_to = max_seq(from.0);
        let mut after: Mark = (since, "\u{10FFFF}".into(), String::new());
        let mut cols = Columns::default();
        let mut taken = 0;
        let mut later = Vec::new();
        loop {
            let (items, end) = changes(from.0, from.1, from.2, to.2, &after, up_to, 2).unwrap();
            let (n, l) = apply(to.0, to.1, to.2, from.2, &items, &mut cols).unwrap();
            taken += n;
            later.extend(l);
            match end {
                Some(m) => after = m,
                None => break,
            }
        }
        taken += apply(to.0, to.1, to.2, from.2, &later, &mut cols).unwrap().0;
        (taken, up_to)
    }

    fn title(conn: &Connection, c: &Cipher, id: &str) -> Option<String> {
        conn.query_row("SELECT title FROM chats WHERE id = ?1", [id], |r| r.get::<_, String>(0)).optional().unwrap().map(|t| c.decrypt(&t).unwrap())
    }

    #[test]
    fn chats_travel_and_the_later_edit_wins() {
        let (a, ca) = pc();
        let (b, cb) = pc();
        a.execute("INSERT INTO chats (id, title, created_at, updated_at) VALUES ('c1', ?1, 1, 1)", [ca.encrypt("Trip plans")]).unwrap();
        a.execute("INSERT INTO messages (id, chat_id, role, content, created_at) VALUES ('m1', 'c1', 'user', ?1, 2)", [ca.encrypt("Where to?")]).unwrap();
        a.execute("INSERT INTO chats (id, title, created_at, updated_at, incognito) VALUES ('secret', 'x', 1, 1, 1)", []).unwrap();
        a.execute("INSERT INTO messages (id, chat_id, role, content, created_at) VALUES ('m2', 'secret', 'user', 'hidden', 2)", []).unwrap();
        crate::db::set_profile(&a, &ca, &crate::db::Profile { name: "Isaac".into(), ..Default::default() }).unwrap();
        crate::db::set(&a, "perf", &serde_json::json!({"mode": "max"})).unwrap();

        let (taken, a_pos) = send((&a, &ca, "A"), (&b, &cb, "B"), 0);
        assert_eq!(taken, 3, "chat, message, profile");
        // Sealed again with B's own key.
        assert_eq!(title(&b, &cb, "c1").as_deref(), Some("Trip plans"));
        let content: String = b.query_row("SELECT content FROM messages WHERE id = 'm1'", [], |r| r.get(0)).unwrap();
        assert!(Cipher::is_encrypted(&content));
        assert_eq!(cb.decrypt(&content).unwrap(), "Where to?");
        assert_eq!(crate::db::profile(&b, &cb).name, "Isaac");
        assert!(title(&b, &cb, "secret").is_none(), "incognito chats stay put");
        assert!(crate::db::get::<serde_json::Value>(&b, "perf").is_none(), "other settings stay put");

        // Nothing new: nothing sent; and B doesn't echo A's rows back.
        assert_eq!(send((&a, &ca, "A"), (&b, &cb, "B"), a_pos).0, 0);
        assert!(!has_news(&b, 0, "A"));

        // Both edit the title; the later edit wins on both PCs.
        a.execute("UPDATE chats SET title = ?1 WHERE id = 'c1'", [ca.encrypt("Old idea")]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        b.execute("UPDATE chats SET title = ?1 WHERE id = 'c1'", [cb.encrypt("Italy trip")]).unwrap();
        let b_pos = 0;
        send((&a, &ca, "A"), (&b, &cb, "B"), a_pos);
        send((&b, &cb, "B"), (&a, &ca, "A"), b_pos);
        assert_eq!(title(&a, &ca, "c1").as_deref(), Some("Italy trip"));
        assert_eq!(title(&b, &cb, "c1").as_deref(), Some("Italy trip"));

        // A deletes the chat: it goes on B, messages with it.
        let pos = max_seq(&a);
        a.execute("DELETE FROM chats WHERE id = 'c1'", []).unwrap();
        send((&a, &ca, "A"), (&b, &cb, "B"), pos);
        assert!(title(&b, &cb, "c1").is_none());
        let left: i64 = b.query_row("SELECT COUNT(*) FROM messages WHERE chat_id = 'c1'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn a_chat_leaving_incognito_brings_its_messages() {
        let (a, ca) = pc();
        let (b, cb) = pc();
        a.execute("INSERT INTO chats (id, title, created_at, updated_at, incognito) VALUES ('c', 't', 1, 1, 1)", []).unwrap();
        a.execute("INSERT INTO messages (id, chat_id, role, content, created_at) VALUES ('m', 'c', 'user', 'hi', 2)", []).unwrap();
        assert_eq!(send((&a, &ca, "A"), (&b, &cb, "B"), 0).0, 0);
        a.execute("UPDATE chats SET incognito = 0 WHERE id = 'c'", []).unwrap();
        assert_eq!(send((&a, &ca, "A"), (&b, &cb, "B"), 0).0, 2);
    }

    #[test]
    fn existing_rows_count_once() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_for_test(&conn);
        conn.execute("INSERT INTO notes (id, data, created_at, updated_at) VALUES ('n', 'x', 1, 7)", []).unwrap();
        // As if the rows were there before sync existed.
        conn.execute("DELETE FROM sync_state", []).unwrap();
        install(&conn).unwrap();
        install(&conn).unwrap();
        let (seq, at): (i64, i64) = conn.query_row("SELECT seq, changed_at FROM sync_state WHERE tbl = 'notes' AND id = 'n'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((seq, at), (1, 7));
        conn.execute("UPDATE notes SET data = 'y' WHERE id = 'n'", []).unwrap();
        assert_eq!(max_seq(&conn), 2);
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Earlier versions of a chat from some point on. Editing a message or
//! asking for another reply puts what came after that point aside as a
//! version, and the chat can be switched back to it. Only the version in
//! use is in `messages`, so everything else (the model, search, sync,
//! export) sees the chat as it reads now; the others are kept here,
//! encrypted, on this PC.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::crypto::Cipher;
use crate::db::{self, Message};

/// The versions after one point in a chat, for the window.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Versions {
    /// The message just before them ("" for the chat's start).
    pub anchor: String,
    /// The first message of the version in use, if it has any yet.
    pub first: Option<String>,
    /// Which one is in use (from 0), and how many there are.
    pub index: usize,
    pub count: usize,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The chat's messages after `anchor` ("" for all of them).
fn tail(all: &[Message], anchor: &str) -> Result<Vec<Message>, String> {
    if anchor.is_empty() {
        return Ok(all.to_vec());
    }
    let at = all.iter().position(|m| m.id == anchor).ok_or("That message isn't in this chat any more.")?;
    Ok(all[at + 1..].to_vec())
}

/// Moves the messages after `anchor` out of the chat into a version of
/// their own. Nothing happens when there are none.
pub fn set_aside(conn: &Connection, c: &Cipher, chat_id: &str, anchor: &str) -> Result<(), String> {
    let gone = tail(&db::messages(conn, c, chat_id), anchor)?;
    let Some(first) = gone.first() else { return Ok(()) };
    let tx = conn.unchecked_transaction().map_err(err)?;
    tx.execute(
        "INSERT INTO message_versions (id, chat_id, anchor, started, messages) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![uuid::Uuid::new_v4().to_string(), chat_id, anchor, first.created_at, c.encrypt(&serde_json::to_string(&gone).map_err(err)?)],
    )
    .map_err(err)?;
    for m in &gone {
        tx.execute("DELETE FROM messages WHERE id = ?1", [&m.id]).map_err(err)?;
    }
    tx.commit().map_err(err)
}

/// The kept versions after `anchor`: (id, started), oldest first.
fn kept(conn: &Connection, chat_id: &str, anchor: &str) -> Vec<(String, i64)> {
    let Ok(mut stmt) = conn.prepare("SELECT id, started FROM message_versions WHERE chat_id = ?1 AND anchor = ?2 ORDER BY started, rowid") else {
        return Vec::new();
    };
    stmt.query_map(params![chat_id, anchor], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Every point in the chat as it reads now that has other versions.
pub fn list(conn: &Connection, c: &Cipher, chat_id: &str) -> Vec<Versions> {
    let all = db::messages(conn, c, chat_id);
    let anchors: Vec<String> = conn
        .prepare("SELECT DISTINCT anchor FROM message_versions WHERE chat_id = ?1")
        .and_then(|mut s| s.query_map([chat_id], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut out = Vec::new();
    for anchor in anchors {
        let Ok(now) = tail(&all, &anchor) else { continue };
        let others = kept(conn, chat_id, &anchor);
        // The one in use sits among the others by when it was started (an
        // empty one, a reply that failed, comes last).
        let started = now.first().map_or(i64::MAX, |m| m.created_at);
        let index = others.iter().filter(|(_, s)| *s <= started).count();
        out.push(Versions { first: now.first().map(|m| m.id.clone()), index, count: others.len() + 1, anchor });
    }
    out.sort_by_key(|v| all.iter().position(|m| v.anchor.is_empty() || m.id == v.anchor));
    out
}

/// Switches the chat to version `to` after `anchor`: the one in use is set
/// aside and that one comes back.
pub fn switch(conn: &Connection, c: &Cipher, chat_id: &str, anchor: &str, to: usize) -> Result<(), String> {
    let now = list(conn, c, chat_id).into_iter().find(|v| v.anchor == anchor).ok_or("There are no other versions here.")?;
    if to == now.index || to >= now.count {
        return Ok(());
    }
    let others = kept(conn, chat_id, anchor);
    let (id, _) = others.get(if to < now.index { to } else { to - 1 }).cloned().ok_or("That version is gone.")?;
    let stored: String = conn.query_row("SELECT messages FROM message_versions WHERE id = ?1", [&id], |r| r.get(0)).map_err(err)?;
    let back: Vec<Message> = serde_json::from_str(&c.decrypt(&stored)?).map_err(err)?;
    set_aside(conn, c, chat_id, anchor)?;
    let tx = conn.unchecked_transaction().map_err(err)?;
    for m in &back {
        db::add_message(&tx, c, m)?;
    }
    tx.execute("DELETE FROM message_versions WHERE id = ?1", [&id]).map_err(err)?;
    tx.execute("UPDATE chats SET updated_at = ?2 WHERE id = ?1", params![chat_id, db::now_ms()]).map_err(err)?;
    tx.commit().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(conn: &Connection, c: &Cipher, id: &str, role: &str, text: &str, at: i64) {
        db::add_message(conn, c, &Message { id: id.into(), chat_id: "c".into(), role: role.into(), content: text.into(), created_at: at, ..Default::default() }).unwrap();
    }

    fn texts(conn: &Connection, c: &Cipher) -> Vec<String> {
        db::messages(conn, c, "c").into_iter().map(|m| m.content).collect()
    }

    /// Another reply and an edited message each keep what was there as a
    /// version; switching brings it back, with the newer one kept in turn.
    #[test]
    fn edits_and_new_replies_keep_the_old_ones() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::init_for_test(&conn);
        let c = Cipher::random();
        conn.execute("INSERT INTO chats (id, title, created_at, updated_at) VALUES ('c', 't', 0, 0)", []).unwrap();
        say(&conn, &c, "u1", "user", "hi", 1);
        say(&conn, &c, "a1", "assistant", "hello", 2);
        say(&conn, &c, "u2", "user", "a joke?", 3);
        say(&conn, &c, "a2", "assistant", "no", 4);
        assert!(list(&conn, &c, "c").is_empty());

        // Another reply to "a joke?".
        set_aside(&conn, &c, "c", "u2").unwrap();
        assert_eq!(texts(&conn, &c), ["hi", "hello", "a joke?"]);
        assert_eq!(list(&conn, &c, "c"), [Versions { anchor: "u2".into(), first: None, index: 1, count: 2 }]);
        say(&conn, &c, "a2b", "assistant", "a pun", 5);
        assert_eq!(list(&conn, &c, "c")[0].first.as_deref(), Some("a2b"));

        // Back to the first reply, and forward again.
        switch(&conn, &c, "c", "u2", 0).unwrap();
        assert_eq!(texts(&conn, &c), ["hi", "hello", "a joke?", "no"]);
        assert_eq!(list(&conn, &c, "c"), [Versions { anchor: "u2".into(), first: Some("a2".into()), index: 0, count: 2 }]);
        switch(&conn, &c, "c", "u2", 1).unwrap();
        assert_eq!(texts(&conn, &c), ["hi", "hello", "a joke?", "a pun"]);

        // Editing the first message sets the whole chat aside; the reply
        // versions inside it come back with it.
        set_aside(&conn, &c, "c", "").unwrap();
        say(&conn, &c, "u1b", "user", "hey", 6);
        let v = list(&conn, &c, "c");
        assert_eq!(v, [Versions { anchor: "".into(), first: Some("u1b".into()), index: 1, count: 2 }]);
        switch(&conn, &c, "c", "", 0).unwrap();
        assert_eq!(texts(&conn, &c), ["hi", "hello", "a joke?", "a pun"]);
        assert_eq!(list(&conn, &c, "c").len(), 2, "both points have versions again");

        // Deleting the chat deletes its versions.
        conn.execute("DELETE FROM chats WHERE id = 'c'", []).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM message_versions", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }
}

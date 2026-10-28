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

/// What going back to a message would take away, for the person to confirm.
#[derive(Debug, Serialize, PartialEq)]
pub struct GoBack {
    /// Messages after the chosen one.
    pub messages: usize,
    /// File changes those turns made that can be put back.
    pub changes: usize,
}

/// From the chosen message on: its index and the turns started in that
/// stretch (each user message that started one).
fn from_message(all: &[Message], message_id: &str) -> Result<(usize, Vec<String>), String> {
    let at = all.iter().position(|m| m.id == message_id).ok_or("That message isn't in this chat any more.")?;
    if all[at].role != "user" {
        return Err("Only a message you wrote can be gone back to.".into());
    }
    Ok((at, all[at..].iter().filter(|m| m.role == "user").map(|m| m.id.clone()).collect()))
}

pub fn go_back_preview(conn: &Connection, c: &Cipher, chat_id: &str, message_id: &str) -> Result<GoBack, String> {
    let all = db::messages(conn, c, chat_id);
    let (at, turns) = from_message(&all, message_id)?;
    let changes = turns.iter().map(|t| db::turn_checkpoints(conn, c, t).len()).sum();
    Ok(GoBack { messages: all.len() - at - 1, changes })
}

/// Goes back to a message you wrote: the files the turns from it on
/// changed are put back (newest first) when `files`, and the message and
/// everything after it is set aside as a version, so it can be switched
/// back to. Returns the message's text, to edit and send again, and notes
/// on anything that couldn't be put back.
pub fn go_back(conn: &Connection, c: &Cipher, chat_id: &str, message_id: &str, files: bool) -> Result<(String, Vec<String>), String> {
    let all = db::messages(conn, c, chat_id);
    let (at, turns) = from_message(&all, message_id)?;
    let mut notes = Vec::new();
    if files {
        for turn in turns.iter().rev() {
            notes.extend(crate::checkpoint::undo_turn(conn, c, turn));
        }
    }
    let anchor = if at == 0 { String::new() } else { all[at - 1].id.clone() };
    set_aside(conn, c, chat_id, &anchor)?;
    Ok((all[at].content.clone(), notes))
}

#[tauri::command(async)]
pub fn go_back_preview_cmd(state: crate::AppStateRef, chat_id: String, message_id: String) -> Result<GoBack, String> {
    let c = state.cipher()?;
    go_back_preview(&state.db.lock().unwrap(), &c, &chat_id, &message_id)
}

#[derive(Debug, Serialize)]
pub struct WentBack {
    pub text: String,
    pub notes: Vec<String>,
}

#[tauri::command(async)]
pub fn go_back_cmd(app: tauri::AppHandle, state: crate::AppStateRef, chat_id: String, message_id: String, files: bool) -> Result<WentBack, String> {
    use tauri::Emitter;
    let c = state.cipher()?;
    if state.generations.lock().unwrap().contains_key(&chat_id) {
        return Err("Stop the reply first.".into());
    }
    let (text, notes) = go_back(&state.db.lock().unwrap(), &c, &chat_id, &message_id, files)?;
    state.contexts.lock().unwrap().remove(&chat_id);
    state.log("chat", &format!("Went back to an earlier message{}", if files { ", putting back file changes" } else { "" }));
    app.emit("chat:changed", serde_json::json!({ "chat_id": chat_id })).ok();
    Ok(WentBack { text, notes })
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

    /// Three turns each change a file; going back to the second puts the
    /// file back as it was before that turn, sets the messages from there
    /// aside (a version to switch back to) and hands back what was asked.
    #[test]
    fn going_back_puts_files_and_messages_back() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = Cipher::random();
        conn.execute("INSERT INTO chats (id, title, created_at, updated_at) VALUES ('c', 't', 0, 0)", []).unwrap();
        let dir = std::env::temp_dir().join(format!("sulcusai-goback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "v0").unwrap();
        let conn = std::sync::Mutex::new(conn);
        for (i, text) in ["v1", "v2", "v3"].iter().enumerate() {
            let turn = format!("u{}", i + 1);
            say(&conn.lock().unwrap(), &c, &turn, "user", &format!("make it {text}"), i as i64 * 10 + 1);
            let rec = crate::checkpoint::Recorder { db: &conn, cipher: &c, dir: dir.join("backups"), chat_id: "c", turn_id: &turn };
            rec.before_write(&file).unwrap();
            std::fs::write(&file, text).unwrap();
            say(&conn.lock().unwrap(), &c, &format!("a{}", i + 1), "assistant", "done", i as i64 * 10 + 2);
        }
        let conn = conn.into_inner().unwrap();
        assert_eq!(go_back_preview(&conn, &c, "c", "u2").unwrap(), GoBack { messages: 3, changes: 2 });
        assert!(go_back_preview(&conn, &c, "c", "a2").is_err(), "only your own messages");
        let (text, notes) = go_back(&conn, &c, "c", "u2", true).unwrap();
        assert_eq!(text, "make it v2");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
        assert_eq!(texts(&conn, &c), ["make it v1", "done"]);
        assert_eq!(list(&conn, &c, "c")[0].anchor, "a1");
        assert_eq!(go_back_preview(&conn, &c, "c", "u1").unwrap(), GoBack { messages: 1, changes: 1 });
        // Without the files, only the messages go.
        go_back(&conn, &c, "c", "u1", false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
        assert!(texts(&conn, &c).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
